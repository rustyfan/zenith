use anyhow::{ensure, Context, Result};
use std::{
    ops::Range,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use zenith_rhi::*;

static NEXT_GRAPH: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferId {
    graph: u64,
    index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageId {
    graph: u64,
    index: usize,
}

#[derive(Clone, Copy)]
pub struct ResourceUse {
    graph: u64,
    index: usize,
    access: Access,
    read: bool,
    write: bool,
    selection: Selection,
}

#[derive(Clone, Copy)]
enum Selection {
    Whole,
    Bytes(u64, u64),
    Image(vk::ImageSubresourceRange),
}

impl ResourceUse {
    pub fn bytes(mut self, range: Range<u64>) -> Self {
        self.selection = Selection::Bytes(range.start, range.end);
        self
    }
    pub fn subresources(mut self, range: vk::ImageSubresourceRange) -> Self {
        self.selection = Selection::Image(range);
        self
    }
}

macro_rules! resource_access {
    ($handle:ty) => {
        impl $handle {
            pub fn read(self, access: Access) -> ResourceUse {
                ResourceUse {
                    graph: self.graph,
                    index: self.index,
                    access,
                    read: true,
                    write: false,
                    selection: Selection::Whole,
                }
            }
            pub fn write(self, access: Access) -> ResourceUse {
                ResourceUse {
                    graph: self.graph,
                    index: self.index,
                    access,
                    read: false,
                    write: true,
                    selection: Selection::Whole,
                }
            }
            pub fn read_write(self, access: Access) -> ResourceUse {
                ResourceUse {
                    graph: self.graph,
                    index: self.index,
                    access,
                    read: true,
                    write: true,
                    selection: Selection::Whole,
                }
            }
        }
    };
}
resource_access!(BufferId);
resource_access!(ImageId);

enum Resource {
    Buffer(Arc<Memory>),
    Image {
        texture: Arc<Texture>,
        view: Arc<TextureView>,
        sampled: Option<Arc<ImageBinding>>,
        storage: Option<Arc<ImageBinding>>,
    },
}

struct Entry {
    resource: Resource,
    transient: bool,
    discard: bool,
    initialized: bool,
}

impl Resource {
    fn size(&self) -> u64 {
        match self {
            Self::Buffer(memory) => memory.size(),
            Self::Image { texture, .. } => {
                let d = texture.desc();
                d.aspect().as_raw().count_ones() as u64 * d.mip_levels as u64 * d.layers as u64
            }
        }
    }
    fn ranges(&self, selection: Selection) -> Result<Vec<Range<u64>>> {
        match (self, selection) {
            (_, Selection::Whole) => Ok(vec![0..self.size()]),
            (Self::Buffer(memory), Selection::Bytes(start, end)) => {
                ensure!(
                    start < end && end <= memory.size(),
                    "graph buffer range out of bounds"
                );
                Ok(vec![start..end])
            }
            (Self::Image { texture, .. }, Selection::Image(range)) => {
                let d = texture.desc();
                ensure!(
                    !range.aspect_mask.is_empty()
                        && d.aspect().contains(range.aspect_mask)
                        && range.level_count > 0
                        && range.layer_count > 0
                        && range
                            .base_mip_level
                            .checked_add(range.level_count)
                            .is_some_and(|end| end <= d.mip_levels)
                        && range
                            .base_array_layer
                            .checked_add(range.layer_count)
                            .is_some_and(|end| end <= d.layers),
                    "graph image range out of bounds"
                );
                let mut ranges = Vec::new();
                let mut aspect_index = 0;
                for bit in 0..32 {
                    let aspect = vk::ImageAspectFlags::from_raw(1 << bit);
                    if !d.aspect().contains(aspect) {
                        continue;
                    }
                    if range.aspect_mask.contains(aspect) {
                        for mip in range.base_mip_level..range.base_mip_level + range.level_count {
                            let start = (aspect_index * d.mip_levels as u64 + mip as u64)
                                * d.layers as u64
                                + range.base_array_layer as u64;
                            ranges.push(start..start + range.layer_count as u64);
                        }
                    }
                    aspect_index += 1;
                }
                Ok(ranges)
            }
            _ => anyhow::bail!("resource range has the wrong type"),
        }
    }
}

#[derive(Default)]
pub struct ResourceCache {
    buffers: Vec<Arc<Memory>>,
    images: Vec<Arc<Texture>>,
}

impl ResourceCache {
    pub fn clear(&mut self) {
        self.buffers.clear();
        self.images.clear();
    }
    pub fn len(&self) -> usize {
        self.buffers.len() + self.images.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

type Execute = Box<dyn FnOnce(&mut PassContext<'_>) -> Result<()>>;
struct Pass {
    name: String,
    uses: Vec<ResourceUse>,
    execute: Execute,
}

pub struct GraphTimings {
    queries: Arc<Timestamps>,
    names: Vec<String>,
}
impl GraphTimings {
    pub fn read(&self) -> Result<Vec<(String, f64)>> {
        let values = self.queries.read()?;
        Ok(self
            .names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    name.clone(),
                    self.queries
                        .milliseconds(values[2 * index], values[2 * index + 1]),
                )
            })
            .collect())
    }
}

pub struct RenderGraphBuilder<'a> {
    id: u64,
    gpu: Arc<Gpu>,
    descriptors: Arc<Descriptors>,
    cache: &'a mut ResourceCache,
    resources: Vec<Entry>,
    passes: Vec<Pass>,
}

impl<'a> RenderGraphBuilder<'a> {
    pub fn new(
        gpu: &Arc<Gpu>,
        descriptors: &Arc<Descriptors>,
        cache: &'a mut ResourceCache,
    ) -> Result<Self> {
        let id = NEXT_GRAPH
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| anyhow::anyhow!("graph ID exhausted"))?;
        Ok(Self {
            id,
            gpu: gpu.clone(),
            descriptors: descriptors.clone(),
            cache,
            resources: Vec::new(),
            passes: Vec::new(),
        })
    }
    pub fn gpu(&self) -> &Arc<Gpu> {
        &self.gpu
    }
    pub fn descriptors(&self) -> &Arc<Descriptors> {
        &self.descriptors
    }

    pub fn create_buffer(&mut self, size: u64, domain: MemoryDomain) -> Result<BufferId> {
        let memory = match self
            .cache
            .buffers
            .iter()
            .position(|m| Arc::strong_count(m) == 1 && m.size() == size && m.domain() == domain)
        {
            Some(index) => self.cache.buffers.swap_remove(index),
            None => self.gpu.allocate(size, domain)?,
        };
        let id = self.import_buffer(memory);
        self.resources[id.index].transient = true;
        self.resources[id.index].initialized = false;
        Ok(id)
    }
    pub fn import_buffer(&mut self, memory: Arc<Memory>) -> BufferId {
        let index = self
            .resources
            .iter()
            .position(|e| matches!(&e.resource, Resource::Buffer(m) if Arc::ptr_eq(m, &memory)))
            .unwrap_or_else(|| {
                let index = self.resources.len();
                self.resources.push(Entry {
                    resource: Resource::Buffer(memory),
                    transient: false,
                    discard: false,
                    initialized: true,
                });
                index
            });
        BufferId {
            graph: self.id,
            index,
        }
    }
    pub fn create_image(&mut self, desc: TextureDesc) -> Result<ImageId> {
        let texture = match self
            .cache
            .images
            .iter()
            .position(|t| Arc::strong_count(t) == 1 && t.desc() == desc)
        {
            Some(index) => self.cache.images.swap_remove(index),
            None => self.gpu.texture(desc)?,
        };
        let id = self.import_image(texture)?;
        let entry = &mut self.resources[id.index];
        entry.transient = true;
        entry.discard = true;
        entry.initialized = false;
        Ok(id)
    }
    pub fn import_image(&mut self, texture: Arc<Texture>) -> Result<ImageId> {
        let index = if let Some(index) = self.resources.iter().position(|e| matches!(&e.resource, Resource::Image { texture: t, .. } if Arc::ptr_eq(t, &texture))) { index } else {
            let index = self.resources.len();
            let view = texture.full_view()?;
            self.resources.push(Entry { resource: Resource::Image { texture, view, sampled: None, storage: None }, transient: false, discard: false, initialized: true });
            index
        };
        Ok(ImageId {
            graph: self.id,
            index,
        })
    }
    pub fn import_frame(&mut self, texture: Arc<Texture>) -> Result<ImageId> {
        let id = self.import_image(texture)?;
        self.resources[id.index].discard = true;
        self.resources[id.index].initialized = false;
        Ok(id)
    }
    pub fn import_sampled(&mut self, binding: Arc<ImageBinding>) -> Result<ImageId> {
        let id = self.import_image(binding.view().texture().clone())?;
        if let Resource::Image { sampled, .. } = &mut self.resources[id.index].resource {
            *sampled = Some(binding);
        }
        Ok(id)
    }
    pub fn image_desc(&self, id: ImageId) -> Result<TextureDesc> {
        Ok(self.export_image(id)?.desc())
    }
    pub fn export_buffer(&self, id: BufferId) -> Result<Arc<Memory>> {
        ensure!(id.graph == self.id, "foreign graph buffer");
        match &self
            .resources
            .get(id.index)
            .context("invalid buffer handle")?
            .resource
        {
            Resource::Buffer(memory) => Ok(memory.clone()),
            _ => anyhow::bail!("not a buffer"),
        }
    }
    pub fn export_image(&self, id: ImageId) -> Result<Arc<Texture>> {
        ensure!(id.graph == self.id, "foreign graph image");
        match &self
            .resources
            .get(id.index)
            .context("invalid image handle")?
            .resource
        {
            Resource::Image { texture, .. } => Ok(texture.clone()),
            _ => anyhow::bail!("not an image"),
        }
    }
    pub fn pass(
        &mut self,
        name: impl Into<String>,
        uses: Vec<ResourceUse>,
        execute: impl FnOnce(&mut PassContext<'_>) -> Result<()> + 'static,
    ) -> Result<()> {
        for usage in &uses {
            ensure!(
                usage.graph == self.id && usage.index < self.resources.len(),
                "foreign graph resource"
            );
            ensure!(
                !usage.access.stages.is_empty(),
                "resource use needs a pipeline stage"
            );
            self.resources[usage.index]
                .resource
                .ranges(usage.selection)?;
        }
        self.passes.push(Pass {
            name: name.into(),
            uses,
            execute: Box::new(execute),
        });
        Ok(())
    }
    pub fn record(self) -> Result<Commands> {
        Ok(self.record_profiled(false)?.0)
    }
    pub fn record_profiled(mut self, profile: bool) -> Result<(Commands, Option<GraphTimings>)> {
        profiling::scope!("Render graph record");
        let mut states: Vec<_> = self
            .resources
            .iter()
            .map(|e| {
                vec![Segment {
                    range: 0..e.resource.size(),
                    state: State {
                        initialized: e.initialized,
                        writer: Access::ALL,
                        readers: Access::NONE,
                    },
                }]
            })
            .collect();
        let mut plans = Vec::new();
        for pass in &self.passes {
            profiling::scope!("Plan pass", &pass.name);
            let uses: Vec<_> = pass
                .uses
                .iter()
                .map(|usage| {
                    Ok(PlannedUse {
                        usage: *usage,
                        ranges: self.resources[usage.index]
                            .resource
                            .ranges(usage.selection)?,
                    })
                })
                .collect::<Result<_>>()?;
            plans.push(
                plan_pass(&mut states, &uses).with_context(|| format!("pass {}", pass.name))?,
            );
        }
        let mut commands = self.gpu.commands()?;
        commands.bind_descriptors(&self.descriptors)?;
        let mut arguments = Arguments::new(&self.gpu, 1024 * 1024)?;
        let timings = if profile && !self.passes.is_empty() {
            let count = u32::try_from(
                self.passes
                    .len()
                    .checked_mul(2)
                    .context("too many graph passes")?,
            )?;
            let queries = self.gpu.timestamps(count)?;
            unsafe {
                commands.reset_timestamps(&queries)?;
            }
            Some(GraphTimings {
                queries,
                names: self.passes.iter().map(|p| p.name.clone()).collect(),
            })
        } else {
            None
        };
        for (index, (pass, barrier)) in self.passes.drain(..).zip(plans).enumerate() {
            profiling::scope!("Record pass", &pass.name);
            if let Some((before, after)) = barrier {
                commands.barrier(before, after)?;
            }
            if let Some(timings) = &timings {
                unsafe {
                    commands.timestamp(&timings.queries, index as u32 * 2)?;
                }
            }
            for usage in &pass.uses {
                let entry = &mut self.resources[usage.index];
                match &entry.resource {
                    Resource::Buffer(memory) => {
                        for range in entry.resource.ranges(usage.selection)? {
                            commands.retain(&memory.slice(range)?)?;
                        }
                    }
                    Resource::Image {
                        texture,
                        sampled,
                        storage,
                        ..
                    } => {
                        if entry.discard {
                            unsafe {
                                commands.transition(
                                    texture,
                                    vk::ImageLayout::UNDEFINED,
                                    vk::ImageLayout::GENERAL,
                                    Access::ALL,
                                    Access::ALL,
                                )?;
                            }
                            entry.discard = false;
                        }
                        commands.retain_texture(texture)?;
                        if let Some(binding) = sampled {
                            commands.retain_image(binding)?;
                        }
                        if let Some(binding) = storage {
                            commands.retain_image(binding)?;
                        }
                    }
                }
            }
            commands
                .label(&pass.name, |commands| {
                    (pass.execute)(&mut PassContext {
                        graph: self.id,
                        resources: &mut self.resources,
                        uses: &pass.uses,
                        arguments: &mut arguments,
                        gpu: &self.gpu,
                        descriptors: &self.descriptors,
                        commands,
                    })
                })
                .with_context(|| format!("recording pass {}", pass.name))?;
            if let Some(timings) = &timings {
                unsafe {
                    commands.timestamp(&timings.queries, index as u32 * 2 + 1)?;
                }
            }
        }
        for entry in self.resources {
            if entry.transient {
                match entry.resource {
                    Resource::Buffer(memory) => self.cache.buffers.push(memory),
                    Resource::Image { texture, .. } => self.cache.images.push(texture),
                }
            }
        }
        Ok((commands, timings))
    }
}

#[derive(Clone, Copy)]
struct State {
    initialized: bool,
    writer: Access,
    readers: Access,
}
#[derive(Clone)]
struct Segment {
    range: Range<u64>,
    state: State,
}
struct PlannedUse {
    usage: ResourceUse,
    ranges: Vec<Range<u64>>,
}
fn merge(a: &mut Access, b: Access) {
    a.stages |= b.stages;
    a.access |= b.access;
}

fn plan_pass(states: &mut [Vec<Segment>], uses: &[PlannedUse]) -> Result<Option<(Access, Access)>> {
    let mut before = Access::NONE;
    let mut after = Access::NONE;
    for (index, segments) in states.iter_mut().enumerate() {
        let relevant: Vec<_> = uses.iter().filter(|u| u.usage.index == index).collect();
        if relevant.is_empty() {
            continue;
        }
        let mut boundaries: Vec<_> = segments
            .iter()
            .flat_map(|s| [s.range.start, s.range.end])
            .chain(
                relevant
                    .iter()
                    .flat_map(|u| u.ranges.iter().flat_map(|r| [r.start, r.end])),
            )
            .collect();
        boundaries.sort_unstable();
        boundaries.dedup();
        let mut next = Vec::new();
        for pair in boundaries.windows(2) {
            let range = pair[0]..pair[1];
            let mut state = segments
                .iter()
                .find(|s| s.range.start <= range.start && range.end <= s.range.end)
                .context("invalid planner range")?
                .state;
            let mut access = Access::NONE;
            let (mut read, mut write) = (false, false);
            for usage in &relevant {
                if usage
                    .ranges
                    .iter()
                    .any(|r| r.start <= range.start && range.end <= r.end)
                {
                    merge(&mut access, usage.usage.access);
                    read |= usage.usage.read;
                    write |= usage.usage.write;
                }
            }
            if !access.stages.is_empty() {
                ensure!(
                    !read || state.initialized,
                    "resource {index} range {range:?} read before initialization"
                );
                let mut source = Access::NONE;
                if write {
                    merge(&mut source, state.writer);
                    merge(&mut source, state.readers);
                    state.writer = access;
                    state.readers = Access::NONE;
                    state.initialized = true;
                } else {
                    if !state.readers.stages.contains(access.stages)
                        || !state.readers.access.contains(access.access)
                    {
                        merge(&mut source, state.writer);
                    }
                    merge(&mut state.readers, access);
                }
                if !source.stages.is_empty() {
                    merge(&mut before, source);
                    merge(&mut after, access);
                }
            }
            next.push(Segment { range, state });
        }
        *segments = next;
    }
    Ok((!after.stages.is_empty()).then_some((before, after)))
}

pub struct PassContext<'a> {
    graph: u64,
    resources: &'a mut [Entry],
    uses: &'a [ResourceUse],
    arguments: &'a mut Arguments,
    pub gpu: &'a Arc<Gpu>,
    pub descriptors: &'a Arc<Descriptors>,
    pub commands: &'a mut Commands,
}

impl PassContext<'_> {
    fn declared(&self, graph: u64, index: usize) -> Result<()> {
        ensure!(
            graph == self.graph && self.uses.iter().any(|u| u.index == index),
            "resource not declared by this pass"
        );
        Ok(())
    }
    pub fn buffer(&self, id: BufferId) -> Result<MemorySlice> {
        self.declared(id.graph, id.index)?;
        self.buffer_range(id, 0..self.resources[id.index].resource.size())
    }
    pub fn buffer_range(&self, id: BufferId, range: Range<u64>) -> Result<MemorySlice> {
        self.declared(id.graph, id.index)?;
        let resource = &self.resources[id.index].resource;
        ensure!(
            self.uses
                .iter()
                .filter(|u| u.index == id.index)
                .any(|u| resource.ranges(u.selection).is_ok_and(|ranges| ranges
                    .iter()
                    .any(|r| r.start <= range.start && range.end <= r.end))),
            "buffer range not declared"
        );
        match resource {
            Resource::Buffer(memory) => memory.slice(range),
            _ => anyhow::bail!("not a buffer"),
        }
    }
    pub fn image(&self, id: ImageId) -> Result<Arc<Texture>> {
        self.declared(id.graph, id.index)?;
        match &self.resources[id.index].resource {
            Resource::Image { texture, .. } => Ok(texture.clone()),
            _ => anyhow::bail!("not an image"),
        }
    }
    pub fn view(&self, id: ImageId) -> Result<Arc<TextureView>> {
        self.declared(id.graph, id.index)?;
        let resource = &self.resources[id.index].resource;
        ensure!(
            self.uses
                .iter()
                .any(|u| u.index == id.index && matches!(u.selection, Selection::Whole)),
            "whole image view not declared"
        );
        match resource {
            Resource::Image { view, .. } => Ok(view.clone()),
            _ => anyhow::bail!("not an image"),
        }
    }
    pub fn view_range(
        &self,
        id: ImageId,
        kind: vk::ImageViewType,
        range: vk::ImageSubresourceRange,
    ) -> Result<Arc<TextureView>> {
        self.declared(id.graph, id.index)?;
        let resource = &self.resources[id.index].resource;
        let requested = resource.ranges(Selection::Image(range))?;
        ensure!(
            requested.iter().all(
                |r| self
                    .uses
                    .iter()
                    .filter(|u| u.index == id.index)
                    .any(|u| resource.ranges(u.selection).is_ok_and(|ranges| ranges
                        .iter()
                        .any(|declared| declared.start <= r.start && r.end <= declared.end)))
            ),
            "image view range not declared"
        );
        match resource {
            Resource::Image { texture, .. } => texture.view(kind, range),
            _ => anyhow::bail!("not an image"),
        }
    }
    pub fn sampled(&mut self, id: ImageId) -> Result<u32> {
        self.binding(id, false)
    }
    pub fn storage(&mut self, id: ImageId) -> Result<u32> {
        self.binding(id, true)
    }
    fn binding(&mut self, id: ImageId, writable: bool) -> Result<u32> {
        self.declared(id.graph, id.index)?;
        match &mut self.resources[id.index].resource {
            Resource::Image {
                view,
                sampled,
                storage,
                ..
            } => {
                let slot = if writable { storage } else { sampled };
                if slot.is_none() {
                    *slot = Some(self.descriptors.image(view, writable)?);
                }
                let binding = slot.as_ref().unwrap();
                self.commands.retain_image(binding)?;
                Ok(binding.index())
            }
            _ => anyhow::bail!("not an image"),
        }
    }
    pub fn sampler(&mut self, sampler: &Arc<Sampler>) -> Result<u32> {
        self.commands.retain_sampler(sampler)?;
        Ok(sampler.index())
    }
    pub fn arguments<T: bytemuck::Pod>(&mut self, value: &T) -> Result<MemorySlice> {
        let memory = self.arguments.push(value)?;
        self.commands.retain(&memory)?;
        Ok(memory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn usage(
        index: usize,
        range: Range<u64>,
        read: bool,
        write: bool,
        access: Access,
    ) -> PlannedUse {
        PlannedUse {
            usage: ResourceUse {
                graph: 1,
                index,
                read,
                write,
                access,
                selection: Selection::Whole,
            },
            ranges: vec![range],
        }
    }
    #[test]
    fn dependency_planning() {
        let mut states = vec![vec![Segment {
            range: 0..64,
            state: State {
                initialized: false,
                writer: Access::NONE,
                readers: Access::NONE,
            },
        }]];
        assert!(plan_pass(
            &mut states,
            &[usage(0, 0..16, true, false, Access::COMPUTE_READ)]
        )
        .is_err());
        plan_pass(
            &mut states,
            &[usage(0, 0..16, false, true, Access::COPY_WRITE)],
        )
        .unwrap();
        assert!(plan_pass(
            &mut states,
            &[usage(0, 16..32, false, true, Access::COPY_WRITE)]
        )
        .unwrap()
        .is_none());
        assert!(plan_pass(
            &mut states,
            &[usage(0, 0..16, false, true, Access::COPY_WRITE)]
        )
        .unwrap()
        .is_some());
        assert!(plan_pass(
            &mut states,
            &[usage(0, 0..16, true, false, Access::COMPUTE_READ)]
        )
        .unwrap()
        .is_some());
        assert!(plan_pass(
            &mut states,
            &[usage(0, 0..16, true, false, Access::COPY_READ)]
        )
        .unwrap()
        .is_some());
        assert!(plan_pass(
            &mut states,
            &[usage(0, 0..16, true, false, Access::COPY_READ)]
        )
        .unwrap()
        .is_none());
        let (before, _) = plan_pass(
            &mut states,
            &[usage(0, 0..16, false, true, Access::COPY_WRITE)],
        )
        .unwrap()
        .unwrap();
        assert!(before
            .stages
            .contains(Access::COMPUTE_READ.stages | Access::COPY_READ.stages));
        assert!(plan_pass(
            &mut states,
            &[usage(0, 0..64, true, false, Access::COPY_READ)]
        )
        .is_err());
    }
}
