use super::{ComputePipeline, Gpu, MemorySlice};
use anyhow::{Context, Result, ensure};
use ash::vk;
use std::any::Any;
use std::sync::Arc;
use zenith_core::log;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    pub stages: vk::PipelineStageFlags2,
    pub access: vk::AccessFlags2,
}

impl Access {
    pub const NONE: Self = Self {
        stages: vk::PipelineStageFlags2::NONE,
        access: vk::AccessFlags2::NONE,
    };
    pub const COPY_READ: Self = Self {
        stages: vk::PipelineStageFlags2::COPY,
        access: vk::AccessFlags2::TRANSFER_READ,
    };
    pub const COPY_WRITE: Self = Self {
        stages: vk::PipelineStageFlags2::ALL_TRANSFER,
        access: vk::AccessFlags2::TRANSFER_WRITE,
    };
    pub const COMPUTE_READ: Self = Self {
        stages: vk::PipelineStageFlags2::COMPUTE_SHADER,
        access: vk::AccessFlags2::SHADER_READ,
    };
    pub const COMPUTE_WRITE: Self = Self {
        stages: vk::PipelineStageFlags2::COMPUTE_SHADER,
        access: vk::AccessFlags2::SHADER_WRITE,
    };
    pub const HOST_READ: Self = Self {
        stages: vk::PipelineStageFlags2::HOST,
        access: vk::AccessFlags2::HOST_READ,
    };
    pub const INDIRECT: Self = Self {
        stages: vk::PipelineStageFlags2::DRAW_INDIRECT,
        access: vk::AccessFlags2::INDIRECT_COMMAND_READ,
    };
    pub const ALL: Self = Self {
        stages: vk::PipelineStageFlags2::ALL_COMMANDS,
        access: vk::AccessFlags2::from_raw(
            vk::AccessFlags2::MEMORY_READ.as_raw() | vk::AccessFlags2::MEMORY_WRITE.as_raw(),
        ),
    };
}

pub struct Commands {
    pub(crate) gpu: Arc<Gpu>,
    pub(crate) raw: vk::CommandBuffer,
    pool: vk::CommandPool,
    pub(crate) queue: usize,
    id: u64,
    memories: Vec<MemorySlice>,
    pub(crate) retained: Vec<Arc<dyn Any + Send + Sync>>,
    pub(crate) rendering: Option<super::raster::RenderingSignature>,
}

impl Gpu {
    pub fn commands(self: &Arc<Self>) -> Result<Commands> {
        self.commands_on(0)
    }

    pub fn commands_on(self: &Arc<Self>, queue: usize) -> Result<Commands> {
        ensure!(queue < self.queues.len(), "requested queue is unavailable");
        let id = self
            .next_recording
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |n| n.checked_add(1),
            )
            .map_err(|_| anyhow::anyhow!("recording identifier overflow"))?;
        let pooled = self.pools.lock().unwrap_or_else(|p| p.into_inner()).pop();
        let (pool, raw) = if let Some(pair) = pooled {
            pair
        } else {
            let pool = unsafe {
                self.raw.create_command_pool(
                    &vk::CommandPoolCreateInfo::default()
                        .queue_family_index(self.queue_family)
                        .flags(vk::CommandPoolCreateFlags::TRANSIENT),
                    None,
                )?
            };
            let allocated = unsafe {
                self.raw.allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default()
                        .command_pool(pool)
                        .level(vk::CommandBufferLevel::PRIMARY)
                        .command_buffer_count(1),
                )
            };
            match allocated {
                Ok(buffers) => (pool, buffers[0]),
                Err(error) => {
                    unsafe {
                        self.raw.destroy_command_pool(pool, None);
                    }
                    return Err(error.into());
                }
            }
        };
        let commands = Commands {
            gpu: self.clone(),
            raw,
            pool,
            queue,
            id,
            memories: Vec::new(),
            retained: Vec::new(),
            rendering: None,
        };
        unsafe {
            self.raw
                .reset_command_pool(pool, vk::CommandPoolResetFlags::empty())?;
            self.raw.begin_command_buffer(
                raw,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
        }
        Ok(commands)
    }
}

impl Commands {
    pub fn label<T>(
        &mut self,
        name: &str,
        record: impl FnOnce(&mut Commands) -> Result<T>,
    ) -> Result<T> {
        let name = std::ffi::CString::new(name)?;
        if let Some(api) = &self.gpu.debug_utils {
            unsafe {
                api.cmd_begin_debug_utils_label(
                    self.raw,
                    &vk::DebugUtilsLabelEXT::default().label_name(&name),
                );
            }
        }
        let result = record(self);
        if let Some(api) = &self.gpu.debug_utils {
            unsafe {
                api.cmd_end_debug_utils_label(self.raw);
            }
        }
        result
    }

    pub fn retain(&mut self, memory: &MemorySlice) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&memory.memory.gpu, &self.gpu),
            "resource belongs to a different device"
        );
        memory
            .memory
            .users
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(memory.offset..memory.offset + memory.size);
        self.memories.push(memory.clone());
        Ok(())
    }

    pub fn barrier(&mut self, before: Access, after: Access) -> Result<()> {
        ensure!(self.rendering.is_none(), "barrier inside rendering");
        let barriers = [vk::MemoryBarrier2::default()
            .src_stage_mask(before.stages)
            .src_access_mask(before.access)
            .dst_stage_mask(after.stages)
            .dst_access_mask(after.access)];
        unsafe {
            self.gpu.raw.cmd_pipeline_barrier2(
                self.raw,
                &vk::DependencyInfo::default().memory_barriers(&barriers),
            );
        }
        Ok(())
    }

    pub fn copy(&mut self, source: &MemorySlice, destination: &MemorySlice) -> Result<()> {
        ensure!(self.rendering.is_none(), "copy inside rendering");
        ensure!(source.size == destination.size, "copy sizes differ");
        if Arc::ptr_eq(&source.memory, &destination.memory) {
            ensure!(
                source.offset + source.size <= destination.offset
                    || destination.offset + destination.size <= source.offset,
                "overlapping copy"
            );
        }
        self.retain(source)?;
        self.retain(destination)?;
        let copies = [vk::BufferCopy::default()
            .src_offset(source.offset)
            .dst_offset(destination.offset)
            .size(source.size)];
        unsafe {
            self.gpu.raw.cmd_copy_buffer(
                self.raw,
                source.memory.raw,
                destination.memory.raw,
                &copies,
            );
        }
        Ok(())
    }

    pub fn fill(&mut self, destination: &MemorySlice, value: u32) -> Result<()> {
        ensure!(
            self.rendering.is_none() && destination.offset % 4 == 0 && destination.size % 4 == 0,
            "invalid fill range or rendering scope"
        );
        self.retain(destination)?;
        unsafe {
            self.gpu.raw.cmd_fill_buffer(
                self.raw,
                destination.memory.raw,
                destination.offset,
                destination.size,
                value,
            );
        }
        Ok(())
    }

    pub(crate) fn push_root(&mut self, vertex: u64, fragment: u64) {
        let roots = [vertex, fragment];
        unsafe {
            self.gpu.heap.cmd_push_data(
                self.raw,
                &vk::PushDataInfoEXT::default().data(
                    vk::HostAddressRangeConstEXT::default().address(bytemuck::cast_slice(&roots)),
                ),
            );
        }
    }

    pub unsafe fn dispatch(
        &mut self,
        pipeline: &Arc<ComputePipeline>,
        root: &MemorySlice,
        groups: [u32; 3],
        reachable: &[MemorySlice],
    ) -> Result<()> {
        ensure!(
            self.rendering.is_none() && Arc::ptr_eq(&pipeline.gpu, &self.gpu),
            "invalid compute device or rendering scope"
        );
        ensure!(
            root.address().value() % 8 == 0 && root.size >= 8,
            "unaligned or empty compute arguments"
        );
        for (i, count) in groups.iter().enumerate() {
            ensure!(
                *count <= self.gpu.limits.max_compute_work_group_count[i],
                "dispatch exceeds workgroup limit"
            );
        }
        self.retain(root)?;
        for memory in reachable {
            self.retain(memory)?;
        }
        self.retained.push(pipeline.clone());
        self.push_root(root.address().value(), 0);
        unsafe {
            self.gpu
                .raw
                .cmd_bind_pipeline(self.raw, vk::PipelineBindPoint::COMPUTE, pipeline.raw);
            self.gpu
                .raw
                .cmd_dispatch(self.raw, groups[0], groups[1], groups[2]);
        }
        Ok(())
    }

    pub unsafe fn dispatch_indirect(
        &mut self,
        pipeline: &Arc<ComputePipeline>,
        root: &MemorySlice,
        indirect: &MemorySlice,
        reachable: &[MemorySlice],
    ) -> Result<()> {
        ensure!(
            self.rendering.is_none() && Arc::ptr_eq(&pipeline.gpu, &self.gpu),
            "invalid compute device or rendering scope"
        );
        ensure!(
            indirect.offset % 4 == 0 && indirect.size >= 12,
            "invalid indirect dispatch range"
        );
        self.retain(root)?;
        self.retain(indirect)?;
        for memory in reachable {
            self.retain(memory)?;
        }
        self.retained.push(pipeline.clone());
        self.push_root(root.address().value(), 0);
        unsafe {
            self.gpu
                .raw
                .cmd_bind_pipeline(self.raw, vk::PipelineBindPoint::COMPUTE, pipeline.raw);
            self.gpu
                .raw
                .cmd_dispatch_indirect(self.raw, indirect.memory.raw, indirect.offset);
        }
        Ok(())
    }

    pub fn submit(self) -> Result<Submission> {
        self.submit_with(&[], &[])
    }

    pub fn submit_after(self, previous: &Submission) -> Result<Submission> {
        ensure!(
            Arc::ptr_eq(&self.gpu, &previous.gpu),
            "submission belongs to another device"
        );
        let waits = [vk::SemaphoreSubmitInfo::default()
            .semaphore(self.gpu.queues[previous.queue].timeline)
            .value(previous.value)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        self.submit_with(&waits, &[])
    }

    pub fn signal_dependency(&mut self, before: Access, after: Access) -> Result<SplitDependency> {
        ensure!(
            self.rendering.is_none(),
            "split dependency inside rendering"
        );
        let event = Arc::new(Event {
            gpu: self.gpu.clone(),
            raw: unsafe {
                self.gpu
                    .raw
                    .create_event(&vk::EventCreateInfo::default(), None)?
            },
        });
        let barriers = [vk::MemoryBarrier2::default()
            .src_stage_mask(before.stages)
            .src_access_mask(before.access)
            .dst_stage_mask(after.stages)
            .dst_access_mask(after.access)];
        unsafe {
            self.gpu.raw.cmd_set_event2(
                self.raw,
                event.raw,
                &vk::DependencyInfo::default().memory_barriers(&barriers),
            );
        }
        self.retained.push(event.clone());
        Ok(SplitDependency {
            event,
            command: self.id,
            before,
            after,
        })
    }

    pub fn wait_dependency(&mut self, dependency: SplitDependency) -> Result<()> {
        ensure!(
            self.rendering.is_none()
                && dependency.command == self.id
                && Arc::ptr_eq(&dependency.event.gpu, &self.gpu),
            "split dependency must be paired within its recording"
        );
        let barriers = [vk::MemoryBarrier2::default()
            .src_stage_mask(dependency.before.stages)
            .src_access_mask(dependency.before.access)
            .dst_stage_mask(dependency.after.stages)
            .dst_access_mask(dependency.after.access)];
        unsafe {
            self.gpu.raw.cmd_wait_events2(
                self.raw,
                &[dependency.event.raw],
                &[vk::DependencyInfo::default().memory_barriers(&barriers)],
            );
        }
        Ok(())
    }

    pub(crate) fn submit_with(
        self,
        waits: &[vk::SemaphoreSubmitInfo<'_>],
        signals: &[vk::SemaphoreSubmitInfo<'_>],
    ) -> Result<Submission> {
        ensure!(self.rendering.is_none(), "rendering has not ended");
        unsafe {
            self.gpu.raw.end_command_buffer(self.raw)?;
        }
        let queue = &self.gpu.queues[self.queue];
        let mut submitted = queue.submitted.lock().unwrap_or_else(|p| p.into_inner());
        let value = submitted.checked_add(1).context("timeline overflow")?;
        let mut signal_infos = signals.to_vec();
        signal_infos.push(
            vk::SemaphoreSubmitInfo::default()
                .semaphore(queue.timeline)
                .value(value)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS),
        );
        let buffers = [vk::CommandBufferSubmitInfo::default().command_buffer(self.raw)];
        let submits = [vk::SubmitInfo2::default()
            .command_buffer_infos(&buffers)
            .wait_semaphore_infos(waits)
            .signal_semaphore_infos(&signal_infos)];
        unsafe {
            self.gpu
                .raw
                .queue_submit2(queue.raw, &submits, vk::Fence::null())?;
        }
        *submitted = value;
        drop(submitted);
        Ok(Submission {
            gpu: self.gpu.clone(),
            value,
            queue: self.queue,
            commands: Some(self),
        })
    }
}

impl Drop for Commands {
    fn drop(&mut self) {
        for memory in &self.memories {
            let mut users = memory
                .memory
                .users
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let index = users
                .iter()
                .position(|range| *range == (memory.offset..memory.offset + memory.size))
                .unwrap();
            users.swap_remove(index);
        }
        self.gpu
            .pools
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((self.pool, self.raw));
    }
}

pub struct Submission {
    gpu: Arc<Gpu>,
    value: u64,
    queue: usize,
    commands: Option<Commands>,
}

impl Submission {
    pub fn value(&self) -> u64 {
        self.value
    }
    pub fn poll(&mut self) -> Result<bool> {
        if self.gpu.completed_on(self.queue)? >= self.value {
            self.commands.take();
            Ok(true)
        } else {
            Ok(false)
        }
    }
    pub fn wait(&mut self, timeout_ns: u64) -> Result<()> {
        self.gpu.wait_on(self.queue, self.value, timeout_ns)?;
        self.commands.take();
        Ok(())
    }
}

impl Drop for Submission {
    fn drop(&mut self) {
        if self.commands.is_some() {
            if let Err(error) = self.gpu.wait_on(self.queue, self.value, 10_000_000_000) {
                log::error!("GPU completion failed: {error}");
                if error.downcast_ref::<vk::Result>() != Some(&vk::Result::ERROR_DEVICE_LOST) {
                    std::mem::forget(self.commands.take());
                }
            }
        }
    }
}

struct Event {
    gpu: Arc<Gpu>,
    raw: vk::Event,
}
impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            self.gpu.raw.destroy_event(self.raw, None);
        }
    }
}
pub struct SplitDependency {
    event: Arc<Event>,
    command: u64,
    before: Access,
    after: Access,
}
