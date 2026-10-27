//! Experiment: coarse height samples from the final density function.
//!
//! This samples terrain solids and approximates the sea-level water plane;
//! it is not authoritative block, aquifer, feature or structure generation.

use crate::generation::generator::VanillaGenerator;
use crate::generation::noise::router::chunk_density_function::ChunkNoiseFunctionBuilderOptions;
use crate::generation::noise::router::chunk_noise_router::ChunkNoiseRouter;
use crate::generation::noise::router::surface_height_sampler::{
    SurfaceHeightEstimateSampler, SurfaceHeightSamplerBuilderOptions,
};
use pumpkin_util::math::vector3::Vector3;

use super::coarse_surface::HeightTile;

/// Samples the same seeded final-density router used for chunk generation,
/// without instantiating a `ProtoChunk` or running biome, feature and light stages.
pub struct DensitySurfaceSampler<'a> {
    router: ChunkNoiseRouter<'a>,
    preliminary: SurfaceHeightEstimateSampler<'a>,
    min_y: i32,
    max_y: i32,
    sea_level: i32,
    vertical_step: i32,
    tiles_since_reset: usize,
}

const CACHE_RESET_TILES: usize = 2048;

impl<'a> DensitySurfaceSampler<'a> {
    pub fn new(generator: &'a VanillaGenerator, vertical_step: i32) -> Self {
        let options = ChunkNoiseFunctionBuilderOptions::new(Vec::new(), Vec::new(), None);
        let shape = &generator.settings.shape;
        Self {
            router: ChunkNoiseRouter::generate(&generator.base_router.noise, &options),
            preliminary: SurfaceHeightEstimateSampler::generate(
                &generator.base_router.surface_estimator,
                &SurfaceHeightSamplerBuilderOptions::new(
                    i32::from(shape.min_y),
                    i32::from(shape.max_y()),
                    shape.vertical_cell_block_count() as usize,
                ),
            ),
            min_y: i32::from(shape.min_y),
            max_y: i32::from(shape.max_y()),
            sea_level: generator.settings.sea_level,
            vertical_step: vertical_step.max(1),
            tiles_since_reset: 0,
        }
    }

    /// An approximate inclusive Y of the highest non-air block.
    /// Fluid/ocean surface is approximated by the sea-level plane, ignoring
    /// aquifers and locally varying fluid levels.
    pub fn estimate_height(&mut self, x: i32, z: i32) -> i32 {
        // The preliminary estimate is cheaply derived from the same seed.
        // Begin above it, avoiding repeated final-density evaluation in
        // empty upper atmosphere. A conservative headroom avoids most
        // clipping, but this is a provisional visual representation only.
        let top = (self.preliminary.estimate_height(x, z) + 96).clamp(self.min_y, self.max_y - 1);
        self.scan_from(x, z, top)
    }

    /// Diagnostic reference search starting at the world height limit.
    pub fn estimate_height_exhaustive(&mut self, x: i32, z: i32) -> i32 {
        self.scan_from(x, z, self.max_y - 1)
    }

    fn scan_from(&mut self, x: i32, z: i32, top: i32) -> i32 {
        let mut y = top;
        while y >= self.min_y {
            let density = self.router.final_density(&Vector3::new(x, y, z));
            if density > 0.0 {
                let upper = (y + self.vertical_step - 1).min(top);
                for candidate in (y + 1..=upper).rev() {
                    if self.router.final_density(&Vector3::new(x, candidate, z)) > 0.0 {
                        return candidate.max(self.sea_level - 1);
                    }
                }
                return y.max(self.sea_level - 1);
            }
            y -= self.vertical_step;
        }
        self.sea_level - 1
    }

    pub fn sample_tile(
        &mut self,
        chunk_x: i32,
        chunk_z: i32,
        step: u8,
    ) -> Result<HeightTile, &'static str> {
        let side = match step {
            4 | 8 | 16 => 16 / usize::from(step) + 1,
            _ => return Err("invalid step"),
        };
        // The preliminary surface estimator caches quart coordinates. Bound
        // its retained memory for long-distance exploration.
        if self.tiles_since_reset == CACHE_RESET_TILES {
            self.preliminary.clear_cached_heights();
            self.tiles_since_reset = 0;
        }
        self.tiles_since_reset += 1;
        let x0 = chunk_x.checked_mul(16).ok_or("x coordinate overflow")?;
        let z0 = chunk_z.checked_mul(16).ok_or("z coordinate overflow")?;
        let mut heights = Vec::with_capacity(side * side);
        for z in 0..side {
            for x in 0..side {
                let bx = x0
                    .checked_add((x as i32) * i32::from(step))
                    .ok_or("x coordinate overflow")?;
                let bz = z0
                    .checked_add((z as i32) * i32::from(step))
                    .ok_or("z coordinate overflow")?;
                heights.push(
                    i16::try_from(self.estimate_height(bx, bz)).map_err(|_| "height overflow")?,
                );
            }
        }
        Ok(HeightTile {
            chunk_x,
            chunk_z,
            step,
            heights: heights.into_boxed_slice(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::generator::VanillaGenerator;
    use pumpkin_data::dimension::Dimension;
    use pumpkin_util::world_seed::Seed;

    #[test]
    fn tiles_are_deterministic_and_share_exact_edges() {
        let generator = VanillaGenerator::new_with_settings(Seed(42), Dimension::OVERWORLD, None);
        let mut sampler = DensitySurfaceSampler::new(&generator, 8);
        let mut repeated = DensitySurfaceSampler::new(&generator, 8);
        for step in [4u8, 8, 16] {
            let a = sampler.sample_tile(-2, 3, step).unwrap();
            let b = sampler.sample_tile(-1, 3, step).unwrap();
            assert_eq!(a, repeated.sample_tile(-2, 3, step).unwrap());
            assert_eq!(HeightTile::decode(&a.encode()), Ok(a.clone()));
            let side = 16 / usize::from(step) + 1;
            for z in 0..side {
                assert_eq!(a.heights[z * side + side - 1], b.heights[z * side]);
            }
        }
    }

    #[test]
    fn no_invalid_coordinates_or_resolution_are_accepted() {
        let generator = VanillaGenerator::new_with_settings(Seed(42), Dimension::OVERWORLD, None);
        let mut sampler = DensitySurfaceSampler::new(&generator, 8);
        assert!(sampler.sample_tile(0, 0, 0).is_err());
        assert!(sampler.sample_tile(0, 0, 2).is_err());
        assert!(sampler.sample_tile(i32::MAX, 0, 4).is_err());
        assert!(sampler.sample_tile(0, i32::MIN, 16).is_err());
    }
}
