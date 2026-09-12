use std::sync::Arc;
use zenith_core::log;
use zenith_rhi::*;

pub fn run(gpu: &Arc<Gpu>) -> anyhow::Result<()> {
    anyhow::ensure!(
        gpu.commands_on(gpu.queue_count()).is_err(),
        "unavailable queue accepted"
    );
    let stale = {
        let mut abandoned = gpu.commands()?;
        abandoned.signal_dependency(Access::COPY_WRITE, Access::COPY_READ)?
    };
    anyhow::ensure!(
        gpu.commands()?.wait_dependency(stale).is_err(),
        "stale split dependency accepted after pool reuse"
    );
    anyhow::ensure!(
        gpu.allocate(0, MemoryDomain::Upload).is_err(),
        "zero allocation accepted"
    );
    anyhow::ensure!(
        gpu.allocate(u64::MAX, MemoryDomain::Device).is_err(),
        "overflow allocation accepted"
    );
    let baseline = gpu.allocation_count()?;
    let start = std::time::Instant::now();
    for i in 0..128u32 {
        let upload = gpu.allocate(64, MemoryDomain::Upload)?;
        let device = gpu.allocate(64, MemoryDomain::Device)?;
        let readback = gpu.allocate(64, MemoryDomain::Readback)?;
        upload.write(3, &[i as u8, 17, 253])?;
        anyhow::ensure!(
            upload.slice(0..65).is_err() && upload.write(63, &[1, 2]).is_err(),
            "out of bounds access accepted"
        );
        {
            let mut abandoned = gpu.commands()?;
            abandoned.retain(&upload.whole())?;
            anyhow::ensure!(
                upload.write(0, &[1]).is_err(),
                "retained allocation accepted CPU write"
            );
        }
        upload.write(0, &[9])?;
        let mut commands = gpu.commands()?;
        commands.label("lifetime/copy", |commands| {
            anyhow::ensure!(
                commands
                    .copy(&upload.slice(0..8)?, &upload.slice(4..12)?)
                    .is_err(),
                "overlapping copy accepted"
            );
            anyhow::ensure!(
                commands.fill(&device.slice(1..5)?, 0).is_err(),
                "unaligned fill accepted"
            );
            commands.fill(&device.whole(), i)?;
            commands.barrier(Access::COPY_WRITE, Access::COPY_WRITE)?;
            commands.fill(&device.whole(), i + 1)?;
            commands.barrier(Access::COPY_WRITE, Access::COPY_WRITE)?;
            commands.copy(&upload.slice(3..6)?, &device.slice(5..8)?)?;
            Ok(())
        })?;
        let first = commands.submit()?;
        let mut commands = gpu.commands_on(if gpu.queue_count() > 1 { 1 } else { 0 })?;
        commands.barrier(Access::COPY_WRITE, Access::COPY_READ)?;
        commands.copy(&device.whole(), &readback.whole())?;
        commands.barrier(Access::COPY_WRITE, Access::HOST_READ)?;
        let mut second = commands.submit_after(&first)?;
        drop(upload);
        drop(device);
        drop(first);
        second.wait(10_000_000_000)?;
        anyhow::ensure!(second.poll()?, "completed submission did not poll ready");
        let mut data = [0u8; 64];
        readback.read(0, &mut data)?;
        anyhow::ensure!(data[5..8] == [i as u8, 17, 253], "unaligned copy mismatch");
    }
    anyhow::ensure!(
        gpu.allocation_count()? == baseline,
        "VMA allocation leak across lifetime stress"
    );
    anyhow::ensure!(
        gpu.wait(gpu.completed_value()? + 1, 0).is_err(),
        "unsignaled timeline did not time out"
    );
    log::info!(
        "PASS: 128 lifetime/retirement cycles, same-access writes, byte copies, submission waits/poll/timeout, rejected bounds; {:.2} ms",
        start.elapsed().as_secs_f64() * 1000.0
    );
    abi(gpu)?;
    Ok(())
}

fn abi(gpu: &Arc<Gpu>) -> anyhow::Result<()> {
    #[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
    #[repr(C)]
    struct Source {
        matrix: [[f32; 4]; 4],
        vector: [f32; 4],
        values: [f32; 4],
    }
    let source = Source {
        matrix: [
            [2.0, 0.0, 0.0, 0.0],
            [0.0, 3.0, 0.0, 0.0],
            [0.0, 0.0, 4.0, 0.0],
            [5.0, 7.0, 11.0, 1.0],
        ],
        vector: [1.0, 2.0, 3.0, 1.0],
        values: [3.0, 7.0, 17.0, 21.0],
    };
    let data = gpu.allocate(std::mem::size_of::<Source>() as u64, MemoryDomain::Upload)?;
    data.write(0, bytemuck::bytes_of(&source))?;
    let child = gpu.allocate(16, MemoryDomain::Device)?;
    let result = gpu.allocate(16, MemoryDomain::Readback)?;
    let indirect = gpu.allocate(12, MemoryDomain::Device)?;
    let mut arguments = Arguments::new(gpu, 1024)?;
    let root = arguments.push(&[
        child.address().value(),
        data.address().value(),
        result.address().value(),
        indirect.address().value(),
    ])?;
    let producer = gpu.compute(&gpu.compile_shader(
        "zenith-rhi/tests/shaders/abi.slang",
        "produce",
        ShaderStage::Compute,
    )?)?;
    let consumer = gpu.compute(&gpu.compile_shader(
        "zenith-rhi/tests/shaders/abi.slang",
        "consume",
        ShaderStage::Compute,
    )?)?;
    let timestamps = gpu.timestamps(2)?;
    let mut commands = gpu.commands()?;
    unsafe {
        commands.reset_timestamps(&timestamps)?;
        commands.timestamp(&timestamps, 0)?;
        commands.dispatch(
            &producer,
            &root,
            [1, 1, 1],
            &[
                child.whole(),
                data.whole(),
                result.whole(),
                indirect.whole(),
            ],
        )?;
    }
    commands.barrier(Access::COMPUTE_WRITE, Access::COMPUTE_READ)?;
    commands.barrier(Access::COMPUTE_WRITE, Access::INDIRECT)?;
    unsafe {
        commands.dispatch_indirect(
            &consumer,
            &child.whole(),
            &indirect.whole(),
            &[data.whole(), result.whole()],
        )?;
        commands.timestamp(&timestamps, 1)?;
    }
    commands.barrier(Access::COMPUTE_WRITE, Access::HOST_READ)?;
    commands.submit()?.wait(10_000_000_000)?;
    let mut actual = [0.0f32; 4];
    result.read(0, bytemuck::cast_slice_mut(&mut actual))?;
    anyhow::ensure!(
        actual == [24.0, 30.0, 40.0, 18.0],
        "pointer/matrix/array ABI mismatch: {actual:?}"
    );
    let times = timestamps.read()?;
    log::info!(
        "PASS: GPU-written root pointers, nested matrix/vector/array ABI, indirect dispatch; GPU {:.4} ms",
        timestamps.milliseconds(times[0], times[1])
    );
    Ok(())
}
