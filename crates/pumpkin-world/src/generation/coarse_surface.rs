//! Experimental distant terrain silhouette from Pumpkin's seeded noise router.
//!
//! Not a canonical chunk: this estimate omits caves, structures, surface
//! rules and features. Keep it out of the normal generation journal.

use crate::generation::generator::VanillaGenerator;
use crate::generation::noise::router::surface_height_sampler::{
    SurfaceHeightEstimateSampler, SurfaceHeightSamplerBuilderOptions,
};

const WIRE_VERSION: u8 = 1;
const CHUNK_BLOCKS: i32 = 16;
const HEADER_BYTES: usize = 10;

/// One provisional, seed-derived height grid with samples on both edges.
/// Including the +16 edge means neighboring tiles share exact endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeightTile {
    pub chunk_x: i32,
    pub chunk_z: i32,
    /// Block spacing: 4, 8 or 16. Never use these samples as canonical voxels.
    pub step: u8,
    pub heights: Box<[i16]>,
}

fn side(step: u8) -> Option<usize> {
    match step {
        4 | 8 | 16 => Some((CHUNK_BLOCKS as usize / usize::from(step)) + 1),
        _ => None,
    }
}

impl HeightTile {
    /// Wire layout: version:u8, step:u8, `chunk_x:i32be`, `chunk_z:i32be`,
    /// followed by (16/step+1)^2 i16be heights in Z-major order.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(HEADER_BYTES + self.heights.len() * 2);
        data.extend_from_slice(&[WIRE_VERSION, self.step]);
        data.extend_from_slice(&self.chunk_x.to_be_bytes());
        data.extend_from_slice(&self.chunk_z.to_be_bytes());
        for &height in &self.heights {
            data.extend_from_slice(&height.to_be_bytes());
        }
        data
    }

    /// Strictly validates the compact prototype payload; corrupted or
    /// unexpected formats are rejected, never treated as missing terrain.
    pub fn decode(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < HEADER_BYTES {
            return Err("truncated height-tile header");
        }
        if data[0] != WIRE_VERSION {
            return Err("unsupported height-tile version");
        }
        let step = data[1];
        let count = side(step).ok_or("invalid height-tile step")?.pow(2);
        if data.len() != HEADER_BYTES + count * 2 {
            return Err("invalid height-tile payload length");
        }
        let chunk_x = i32::from_be_bytes(data[2..6].try_into().map_err(|_| "invalid x")?);
        let chunk_z = i32::from_be_bytes(data[6..10].try_into().map_err(|_| "invalid z")?);
        let heights = data[HEADER_BYTES..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|bytes| i16::from_be_bytes([bytes[0], bytes[1]]))
            .collect();
        Ok(Self {
            chunk_x,
            chunk_z,
            step,
            heights,
        })
    }
}

/// Reuse one noise sampler across many distant tiles on the same worker.
/// This avoids initializing a new router for each 16x16 column.
pub struct CoarseSurfaceSampler<'a> {
    noise: SurfaceHeightEstimateSampler<'a>,
    tiles_since_reset: usize,
}

/// Keep the sampler's quart-aligned noise cache bounded on 512-chunk discs.
const CACHE_RESET_TILES: usize = 2048;

impl<'a> CoarseSurfaceSampler<'a> {
    #[must_use]
    pub fn new(generator: &'a VanillaGenerator) -> Self {
        let shape = &generator.settings.shape;
        let options = SurfaceHeightSamplerBuilderOptions::new(
            i32::from(shape.min_y),
            i32::from(shape.max_y()),
            shape.vertical_cell_block_count() as usize,
        );
        Self {
            noise: SurfaceHeightEstimateSampler::generate(
                &generator.base_router.surface_estimator,
                &options,
            ),
            tiles_since_reset: 0,
        }
    }

    /// A tile is a hint for coarse visualisation, not a canonical chunk.
    pub fn sample_tile(
        &mut self,
        chunk_x: i32,
        chunk_z: i32,
        step: u8,
    ) -> Result<HeightTile, &'static str> {
        let side = side(step).ok_or("invalid height-tile step")?;
        if self.tiles_since_reset == CACHE_RESET_TILES {
            self.noise.clear_cached_heights();
            self.tiles_since_reset = 0;
        }
        self.tiles_since_reset += 1;
        let x0 = chunk_x.checked_mul(CHUNK_BLOCKS).ok_or("x outside world")?;
        let z0 = chunk_z.checked_mul(CHUNK_BLOCKS).ok_or("z outside world")?;
        let mut heights = Vec::with_capacity(side * side);
        for iz in 0..side {
            let z = z0
                .checked_add((iz as i32) * i32::from(step))
                .ok_or("z outside world")?;
            for ix in 0..side {
                let x = x0
                    .checked_add((ix as i32) * i32::from(step))
                    .ok_or("x outside world")?;
                let y = self.noise.estimate_height(x, z);
                heights.push(i16::try_from(y).map_err(|_| "height outside i16 range")?);
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
    use crate::generation::generator::{VanillaGenerator, WorldGenerator};
    use pumpkin_data::dimension::Dimension;
    use pumpkin_util::world_seed::Seed;

    #[test]
    fn payload_roundtrip_rejects_bad_dimensions_and_trailing_bytes() {
        let tile = HeightTile {
            chunk_x: -31,
            chunk_z: 2000,
            step: 8,
            heights: (0..9).map(|n| (n * 3 - 50) as i16).collect(),
        };
        let encoded = tile.encode();
        assert_eq!(&encoded[0..10], &[1, 8, 255, 255, 255, 225, 0, 0, 7, 208]);
        assert_eq!(HeightTile::decode(&encoded), Ok(tile));
        assert!(HeightTile::decode(&encoded[..encoded.len() - 1]).is_err());
        let mut extended = encoded.clone();
        extended.push(0);
        assert!(HeightTile::decode(&extended).is_err());
        let mut broken_step = encoded.clone();
        broken_step[1] = 0;
        assert!(HeightTile::decode(&broken_step).is_err());
        let mut broken_version = encoded;
        broken_version[0] = 2;
        assert!(HeightTile::decode(&broken_version).is_err());
    }

    #[test]
    fn real_noise_is_seed_deterministic_and_neighbor_edges_match() {
        let generator = WorldGenerator::Noise(Box::new(VanillaGenerator::new_with_settings(
            Seed(42),
            Dimension::OVERWORLD,
            None,
        )));
        let WorldGenerator::Noise(generator) = &generator else {
            unreachable!()
        };
        let mut a = CoarseSurfaceSampler::new(generator);
        let mut b = CoarseSurfaceSampler::new(generator);
        for step in [4, 8, 16] {
            let left = a.sample_tile(-2, 3, step).unwrap();
            let right = a.sample_tile(-1, 3, step).unwrap();
            assert_eq!(left, HeightTile::decode(&left.encode()).unwrap());
            let side = super::side(step).unwrap();
            for z in 0..side {
                assert_eq!(left.heights[z * side + side - 1], right.heights[z * side]);
            }
            assert_eq!(left, b.sample_tile(-2, 3, step).unwrap());
        }
        assert!(a.sample_tile(i32::MAX, 0, 4).is_err());
        assert!(a.sample_tile(0, 0, 1).is_err());
    }
}
