use super::Gpu;
use anyhow::{Context, Result, ensure};
use ash::vk;
use std::ops::Range;
use std::sync::{Arc, Mutex};
use vk_mem::Alloc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryDomain {
    Upload,
    Device,
    Readback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(transparent)]
pub struct GpuAddress(pub(crate) u64);

impl GpuAddress {
    pub fn value(self) -> u64 {
        self.0
    }
}

pub(crate) fn align_up(value: u64, alignment: u64) -> Result<u64> {
    ensure!(
        alignment.is_power_of_two(),
        "alignment must be a nonzero power of two"
    );
    Ok(value
        .checked_add(alignment - 1)
        .context("alignment overflow")?
        & !(alignment - 1))
}

pub struct Memory {
    pub(crate) gpu: Arc<Gpu>,
    pub(crate) raw: vk::Buffer,
    pub(crate) allocation: vk_mem::Allocation,
    pub(crate) mapped: usize,
    size: u64,
    address: GpuAddress,
    pub(crate) users: Mutex<Vec<Range<u64>>>,
    domain: MemoryDomain,
    pub(crate) usage: vk::BufferUsageFlags,
}

impl Gpu {
    pub fn allocate(self: &Arc<Self>, size: u64, domain: MemoryDomain) -> Result<Arc<Memory>> {
        self.allocate_buffer(
            size,
            domain,
            vk::BufferUsageFlags::TRANSFER_SRC
                | vk::BufferUsageFlags::TRANSFER_DST
                | vk::BufferUsageFlags::STORAGE_BUFFER
                | vk::BufferUsageFlags::INDEX_BUFFER
                | vk::BufferUsageFlags::INDIRECT_BUFFER
                | vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR,
            16,
        )
    }

    pub(crate) fn allocate_buffer(
        self: &Arc<Self>,
        size: u64,
        domain: MemoryDomain,
        usage: vk::BufferUsageFlags,
        alignment: u64,
    ) -> Result<Arc<Memory>> {
        ensure!(
            size > 0 && size <= isize::MAX as u64,
            "invalid allocation size"
        );
        ensure!(alignment.is_power_of_two(), "invalid allocation alignment");
        let info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(usage | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let allocation_info = vk_mem::AllocationCreateInfo {
            usage: match domain {
                MemoryDomain::Device => vk_mem::MemoryUsage::AutoPreferDevice,
                _ => vk_mem::MemoryUsage::Auto,
            },
            flags: match domain {
                MemoryDomain::Device => vk_mem::AllocationCreateFlags::empty(),
                MemoryDomain::Upload => {
                    vk_mem::AllocationCreateFlags::MAPPED
                        | vk_mem::AllocationCreateFlags::HOST_ACCESS_SEQUENTIAL_WRITE
                }
                MemoryDomain::Readback => {
                    vk_mem::AllocationCreateFlags::MAPPED
                        | vk_mem::AllocationCreateFlags::HOST_ACCESS_RANDOM
                }
            },
            ..Default::default()
        };
        let (raw, mut allocation) = unsafe {
            self.allocator()
                .create_buffer_with_alignment(&info, &allocation_info, alignment)?
        };
        let mapped = self
            .allocator()
            .get_allocation_info(&allocation)
            .mapped_data as usize;
        let address = unsafe {
            self.raw
                .get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(raw))
        };
        if address == 0 || (domain != MemoryDomain::Device && mapped == 0) {
            unsafe {
                self.allocator().destroy_buffer(raw, &mut allocation);
            }
            anyhow::bail!("allocation has no device address or requested host mapping");
        }
        Ok(Arc::new(Memory {
            gpu: self.clone(),
            raw,
            allocation,
            mapped,
            size,
            address: GpuAddress(address),
            users: Mutex::new(Vec::new()),
            domain,
            usage,
        }))
    }
}

impl Memory {
    pub fn domain(&self) -> MemoryDomain {
        self.domain
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn address(&self) -> GpuAddress {
        self.address
    }
    pub fn slice(self: &Arc<Self>, range: Range<u64>) -> Result<MemorySlice> {
        ensure!(
            range.start < range.end && range.end <= self.size,
            "memory slice out of bounds"
        );
        Ok(MemorySlice {
            memory: self.clone(),
            offset: range.start,
            size: range.end - range.start,
        })
    }
    pub fn whole(self: &Arc<Self>) -> MemorySlice {
        MemorySlice {
            memory: self.clone(),
            offset: 0,
            size: self.size,
        }
    }

    pub fn write(&self, offset: u64, data: &[u8]) -> Result<()> {
        let users = self.users.lock().unwrap_or_else(|p| p.into_inner());
        ensure!(
            self.mapped != 0
                && offset
                    .checked_add(data.len() as u64)
                    .is_some_and(|end| end <= self.size),
            "unmapped memory or write out of bounds"
        );
        let atom = self.gpu.limits.non_coherent_atom_size;
        let start = offset & !(atom - 1);
        let end = align_up(offset + data.len() as u64, atom)?;
        ensure!(
            !users
                .iter()
                .any(|range| start < range.end && range.start < end),
            "memory range is retained by recorded or submitted GPU work"
        );
        if !data.is_empty() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    (self.mapped as *mut u8).add(offset as usize),
                    data.len(),
                );
            }
            self.gpu
                .allocator()
                .flush_allocation(&self.allocation, offset, data.len() as u64)?;
        }
        Ok(())
    }

    pub fn read(&self, offset: u64, data: &mut [u8]) -> Result<()> {
        let users = self.users.lock().unwrap_or_else(|p| p.into_inner());
        ensure!(
            self.mapped != 0
                && offset
                    .checked_add(data.len() as u64)
                    .is_some_and(|end| end <= self.size),
            "unmapped memory or read out of bounds"
        );
        let atom = self.gpu.limits.non_coherent_atom_size;
        let start = offset & !(atom - 1);
        let end = align_up(offset + data.len() as u64, atom)?;
        ensure!(
            !users
                .iter()
                .any(|range| start < range.end && range.start < end),
            "memory range is retained by recorded or submitted GPU work"
        );
        if !data.is_empty() {
            self.gpu.allocator().invalidate_allocation(
                &self.allocation,
                offset,
                data.len() as u64,
            )?;
            unsafe {
                std::ptr::copy_nonoverlapping(
                    (self.mapped as *const u8).add(offset as usize),
                    data.as_mut_ptr(),
                    data.len(),
                );
            }
        }
        Ok(())
    }

    pub fn host_write_alignment(&self) -> u64 {
        self.gpu.limits.non_coherent_atom_size
    }

    pub fn host_memory_properties(&self) -> vk::MemoryPropertyFlags {
        let info = self.gpu.allocator().get_allocation_info(&self.allocation);
        unsafe {
            self.gpu
                .instance
                .raw
                .get_physical_device_memory_properties(self.gpu.physical)
        }
        .memory_types[info.memory_type as usize]
            .property_flags
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        unsafe {
            self.gpu
                .allocator()
                .destroy_buffer(self.raw, &mut self.allocation);
        }
    }
}

#[derive(Clone)]
pub struct MemorySlice {
    pub(crate) memory: Arc<Memory>,
    pub(crate) offset: u64,
    pub(crate) size: u64,
}

impl MemorySlice {
    pub fn address(&self) -> GpuAddress {
        GpuAddress(self.memory.address.0 + self.offset)
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn slice(&self, range: Range<u64>) -> Result<Self> {
        ensure!(
            range.start < range.end && range.end <= self.size,
            "memory subslice out of bounds"
        );
        self.memory
            .slice(self.offset + range.start..self.offset + range.end)
    }
}

pub struct Arguments {
    memory: Arc<Memory>,
    cursor: u64,
}

impl Arguments {
    pub fn new(gpu: &Arc<Gpu>, capacity: u64) -> Result<Self> {
        Ok(Self {
            memory: gpu.allocate(capacity, MemoryDomain::Upload)?,
            cursor: 0,
        })
    }
    pub fn push<T: bytemuck::Pod>(&mut self, value: &T) -> Result<MemorySlice> {
        let start = align_up(
            self.cursor,
            (std::mem::align_of::<T>().max(16) as u64)
                .max(self.memory.gpu.limits.non_coherent_atom_size),
        )?;
        let end = start
            .checked_add(std::mem::size_of::<T>() as u64)
            .context("argument size overflow")?;
        let slice = self.memory.slice(start..end)?;
        self.memory.write(start, bytemuck::bytes_of(value))?;
        self.cursor = end;
        Ok(slice)
    }
}

#[cfg(test)]
mod tests {
    use super::align_up;
    #[test]
    fn checked_alignment() {
        assert_eq!(align_up(17, 16).unwrap(), 32);
        assert!(align_up(u64::MAX, 16).is_err());
        assert!(align_up(1, 0).is_err());
        assert!(align_up(1, 3).is_err());
    }
}
