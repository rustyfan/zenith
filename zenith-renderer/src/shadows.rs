use anyhow::Result;
use std::sync::Arc;
use zenith_rhi::*;

#[derive(Default)]
pub(crate) struct SceneAcceleration {
    instances: Vec<AccelerationInstance>,
    tlas: Option<Arc<AccelerationStructure>>,
    pending: Vec<Submission>,
}

impl SceneAcceleration {
    pub fn prepare(
        &mut self,
        gpu: &Arc<Gpu>,
        instances: Vec<AccelerationInstance>,
    ) -> Result<Option<Arc<AccelerationStructure>>> {
        let mut index = 0;
        while index < self.pending.len() {
            if self.pending[index].poll()? {
                self.pending.swap_remove(index);
            } else {
                index += 1;
            }
        }
        if instances.len() == self.instances.len()
            && instances
                .iter()
                .zip(&self.instances)
                .all(|(a, b)| Arc::ptr_eq(&a.blas, &b.blas) && a.transform == b.transform)
        {
            return Ok(self.tlas.clone());
        }
        let tlas = if instances.is_empty() {
            None
        } else {
            let mut commands = gpu.commands()?;
            let tlas = unsafe { commands.build_tlas(&instances)? };
            commands.barrier(Access::AS_BUILD_WRITE, Access::AS_FRAGMENT_READ)?;
            self.pending.push(commands.submit()?);
            Some(tlas)
        };
        self.instances = instances;
        self.tlas = tlas;
        Ok(self.tlas.clone())
    }
}

pub(crate) fn instance_transform(model: [f32; 16]) -> [[f32; 4]; 3] {
    std::array::from_fn(|row| std::array::from_fn(|column| model[column * 4 + row]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Mat4, Vec3};

    #[test]
    fn instance_transform_preserves_world_positions() {
        let model = Mat4::from_translation(Vec3::new(7.0, -3.0, 2.0))
            * Mat4::from_rotation_z(0.7)
            * Mat4::from_scale(Vec3::new(-2.0, 0.5, 3.0));
        let rows = instance_transform(model.to_cols_array());
        for point in [
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(1.0, 2.0, 3.0),
        ] {
            let input = point.extend(1.0).to_array();
            let transformed = Vec3::from_array(std::array::from_fn(|i| {
                rows[i].iter().zip(input).map(|(a, b)| a * b).sum()
            }));
            assert!((transformed - model.transform_point3(point)).length() < 1e-5);
        }
    }
}
