use super::{Commands, Gpu};
use anyhow::{Result, ensure};
use ash::vk;
use std::sync::Arc;

pub struct Timestamps {
    gpu: Arc<Gpu>,
    raw: vk::QueryPool,
    count: u32,
    mask: u64,
}
impl Gpu {
    pub fn timestamps(self: &Arc<Self>, count: u32) -> Result<Arc<Timestamps>> {
        zenith_core::profile::scope!("Create timestamp pool");
        let queues = unsafe {
            self.instance
                .raw
                .get_physical_device_queue_family_properties(self.physical)
        };
        let bits = queues[self.queue_family as usize].timestamp_valid_bits;
        ensure!(
            count > 0 && bits > 0,
            "timestamp queries unsupported or empty"
        );
        let raw = unsafe {
            self.raw.create_query_pool(
                &vk::QueryPoolCreateInfo::default()
                    .query_type(vk::QueryType::TIMESTAMP)
                    .query_count(count),
                None,
            )?
        };
        Ok(Arc::new(Timestamps {
            gpu: self.clone(),
            raw,
            count,
            mask: if bits == 64 {
                u64::MAX
            } else {
                (1u64 << bits) - 1
            },
        }))
    }
}
impl Timestamps {
    pub fn read(&self) -> Result<Vec<u64>> {
        let mut values = vec![0u64; self.count as usize];
        unsafe {
            self.gpu.raw.get_query_pool_results(
                self.raw,
                0,
                &mut values,
                vk::QueryResultFlags::TYPE_64,
            )?;
        }
        Ok(values)
    }
    pub fn milliseconds(&self, before: u64, after: u64) -> f64 {
        (after.wrapping_sub(before) & self.mask) as f64 * self.gpu.limits.timestamp_period as f64
            / 1_000_000.0
    }
}
impl Drop for Timestamps {
    fn drop(&mut self) {
        unsafe {
            self.gpu.raw.destroy_query_pool(self.raw, None);
        }
    }
}
impl Commands {
    pub unsafe fn reset_timestamps(&mut self, timestamps: &Arc<Timestamps>) -> Result<()> {
        ensure!(
            self.rendering.is_none() && Arc::ptr_eq(&self.gpu, &timestamps.gpu),
            "invalid timestamp reset"
        );
        self.retained.push(timestamps.clone());
        unsafe {
            self.gpu
                .raw
                .cmd_reset_query_pool(self.raw, timestamps.raw, 0, timestamps.count);
        }
        Ok(())
    }
    pub unsafe fn timestamp(&mut self, timestamps: &Arc<Timestamps>, index: u32) -> Result<()> {
        ensure!(
            index < timestamps.count && Arc::ptr_eq(&self.gpu, &timestamps.gpu),
            "invalid timestamp index or device"
        );
        self.retained.push(timestamps.clone());
        unsafe {
            self.gpu.raw.cmd_write_timestamp2(
                self.raw,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                timestamps.raw,
                index,
            );
        }
        Ok(())
    }
}
