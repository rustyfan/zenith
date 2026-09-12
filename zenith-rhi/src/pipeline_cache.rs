use super::{Blend, ComputePipeline, RasterPipeline};
use ash::vk;
use std::{collections::HashMap, sync::Weak};

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum PipelineKey {
    Compute {
        code: Vec<u32>,
        specialization: Vec<(u32, u32)>,
    },
    Raster {
        vertex: Vec<u32>,
        fragment: Vec<u32>,
        vertex_specialization: Vec<(u32, u32)>,
        fragment_specialization: Vec<(u32, u32)>,
        colors: Vec<vk::Format>,
        depth: vk::Format,
        stencil: vk::Format,
        samples: vk::SampleCountFlags,
        topology: vk::PrimitiveTopology,
        blend: Vec<Blend>,
        dynamic_blend: bool,
    },
}

pub(crate) enum CachedPipeline {
    Compute(Weak<ComputePipeline>),
    Raster(Weak<RasterPipeline>),
}

impl CachedPipeline {
    fn live(&self) -> bool {
        match self {
            Self::Compute(p) => p.strong_count() > 0,
            Self::Raster(p) => p.strong_count() > 0,
        }
    }
}

#[derive(Default)]
pub(crate) struct PipelineCache {
    pub entries: HashMap<PipelineKey, CachedPipeline>,
}
impl PipelineCache {
    pub fn collect(&mut self) {
        self.entries.retain(|_, pipeline| pipeline.live());
    }
    pub fn len(&self) -> usize {
        self.entries.values().filter(|p| p.live()).count()
    }
}
