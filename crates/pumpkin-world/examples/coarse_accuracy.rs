//! Compare provisional terrain heights against actual Pumpkin chunk generation.
//! This is a reproducible *accuracy* audit, not a speed benchmark.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::panic
)]

use pumpkin_data::dimension::Dimension;
use pumpkin_data::{Block, BlockState, BlockStateId, Mirror, Rotation};
use pumpkin_util::HeightMap;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::world_seed::Seed;
use pumpkin_world::chunk::ChunkHeightmapType;
use pumpkin_world::chunk_system::{Chunk, StagedChunkEnum, generate_single_chunk};
use pumpkin_world::generation::coarse_surface::{CoarseSurfaceSampler, HeightTile};
use pumpkin_world::generation::get_world_gen;
use pumpkin_world::world::{BlockAccessor, WorldPortalExt};
use std::time::Instant;

struct Registry;
impl WorldPortalExt for Registry {
    fn can_place_at(
        &self,
        _block: &Block,
        _state: &BlockState,
        _accessor: &dyn BlockAccessor,
        _position: &BlockPos,
    ) -> bool {
        true
    }
    fn mirror(&self, block: &Block, state: BlockStateId, mirror: Mirror) -> &'static BlockState {
        block.mirror(state, mirror)
    }
    fn rotate(
        &self,
        block: &Block,
        state: BlockStateId,
        rotation: Rotation,
    ) -> &'static BlockState {
        block.rotate(state, rotation)
    }
    fn spawn_mobs_for_chunk_generation(
        &self,
        _cache: &mut dyn pumpkin_world::generation::proto_chunk::GenerationCache,
        _biome: &'static pumpkin_data::chunk::Biome,
        _x: i32,
        _z: i32,
    ) {
    }
}

struct Stats {
    errors: Vec<u16>,
    pairs: Vec<(i32, i32)>,
    signed: i64,
    within: [usize; 5],
    largest: (i32, i32, u8, u8, i32, i32),
}
impl Stats {
    const fn new() -> Self {
        Self {
            errors: Vec::new(),
            pairs: Vec::new(),
            signed: 0,
            within: [0; 5],
            largest: (0, 0, 0, 0, 0, 0),
        }
    }
    fn add(&mut self, predicted: i32, actual: i32, cx: i32, cz: i32, x: u8, z: u8) {
        let error = (predicted - actual).unsigned_abs() as u16;
        self.errors.push(error);
        self.pairs.push((predicted, actual));
        self.signed += i64::from(predicted - actual);
        for (index, tolerance) in [0, 1, 2, 4, 8].into_iter().enumerate() {
            if u32::from(error) <= tolerance {
                self.within[index] += 1;
            }
        }
        if i32::from(error) > (self.largest.4 - self.largest.5).abs() {
            self.largest = (cx, cz, x, z, predicted, actual);
        }
    }
    fn print(&mut self, label: &str) {
        self.errors.sort_unstable();
        let n = self.errors.len();
        let mae = self.errors.iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64;
        let p = |q: f64| self.errors[(((n - 1) as f64 * q).round() as usize).min(n - 1)];
        let mean_pred = self.pairs.iter().map(|(p, _)| *p as f64).sum::<f64>() / n as f64;
        let mean_actual = self.pairs.iter().map(|(_, a)| *a as f64).sum::<f64>() / n as f64;
        let (mut cov, mut var_pred, mut var_actual) = (0.0, 0.0, 0.0);
        for &(pred, actual) in &self.pairs {
            let dp = f64::from(pred) - mean_pred;
            let da = f64::from(actual) - mean_actual;
            cov += dp * da;
            var_pred += dp * dp;
            var_actual += da * da;
        }
        let correlation = cov / (var_pred * var_actual).sqrt();
        let offset = (mean_actual - mean_pred).round() as i32;
        let offset_mae = self
            .pairs
            .iter()
            .map(|&(p, a)| ((p + offset - a).abs()) as f64)
            .sum::<f64>()
            / n as f64;
        println!(
            "height correlation r={correlation:.3}; mean predicted={mean_pred:.1}, mean actual={mean_actual:.1}, after global {offset:+} block calibration MAE={offset_mae:.2}"
        );
        println!(
            "{label}: samples={n}, MAE={mae:.2} blocks, median={} blocks, p95={} blocks, p99={} blocks, max={} blocks, bias={:.2} blocks; within [0,1,2,4,8]={:?}%; worst=({},{}) local({},{}) estimated={} actual={}",
            p(0.5),
            p(0.95),
            p(0.99),
            self.errors[n - 1],
            self.signed as f64 / n as f64,
            self.within
                .map(|v| (v as f64 * 100.0 / n as f64 * 10.0).round() / 10.0),
            self.largest.0,
            self.largest.1,
            self.largest.2,
            self.largest.3,
            self.largest.4,
            self.largest.5,
        );
    }
}
fn interpolated_height(tile: &HeightTile, x: u8, z: u8) -> i32 {
    let step = tile.step as usize;
    let side = 16 / step + 1;
    let x = usize::from(x);
    let z = usize::from(z);
    let x0 = x / step;
    let x1 = (x0 + 1).min(side - 1);
    let z0 = z / step;
    let z1 = (z0 + 1).min(side - 1);
    let dx = (x % step) as i32;
    let dz = (z % step) as i32;
    let s = step as i32;
    let a = i32::from(tile.heights[z0 * side + x0]);
    let b = i32::from(tile.heights[z0 * side + x1]);
    let c = i32::from(tile.heights[z1 * side + x0]);
    let d = i32::from(tile.heights[z1 * side + x1]);
    (a * (s - dx) * (s - dz) + b * dx * (s - dz) + c * (s - dx) * dz + d * dx * dz) / (s * s)
}
fn main() {
    // A mix of near-origin and distant chunks avoids exclusively measuring
    // one sea-level or one smooth patch.
    let positions = [
        (0, 0),
        (1, 0),
        (0, 1),
        (-1, -1),
        (3, 7),
        (7, 3),
        (-9, 12),
        (22, 19),
        (-45, 21),
        (79, -83),
        (129, 128),
        (-128, -129),
        (207, -53),
        (-281, 90),
        (512, 512),
        (-512, 319),
    ];
    let registry = Registry;
    for (seed, stage) in [
        (42i64, StagedChunkEnum::Surface),
        (42i64, StagedChunkEnum::Full),
        (3_676_290_170_339_937_829i64, StagedChunkEnum::Surface),
        (3_676_290_170_339_937_829i64, StagedChunkEnum::Full),
    ] {
        let generator = get_world_gen(
            Seed(seed as u64),
            Dimension::OVERWORLD,
            false,
            Vec::new(),
            String::new(),
        );
        let pumpkin_world::generation::generator::WorldGenerator::Noise(native) =
            generator.as_ref()
        else {
            panic!("noise generator required")
        };
        let mut sampler = CoarseSurfaceSampler::new(native);
        let mut results = [Stats::new(), Stats::new(), Stats::new()];
        let started = Instant::now();
        let limit = positions.len();
        for (i, &(cx, cz)) in positions.iter().take(limit).enumerate() {
            let chunk = generate_single_chunk(generator.as_ref(), &registry, cx, cz, stage);
            let tiles = [4u8, 8, 16].map(|step| sampler.sample_tile(cx, cz, step).unwrap());
            for x in 0..16 {
                for z in 0..16 {
                    // Both values refer to the top solid/water block (inclusive).
                    // ProtoChunk's getter is exclusive. ChunkHeightmaps returns
                    // inclusive Y by construction.
                    let actual = match &chunk {
                        Chunk::Proto(proto) => proto.get_top_y(&HeightMap::WorldSurface, x, z) - 1,
                        Chunk::Level(level) => level.calculate_heightmap().get(
                            ChunkHeightmapType::WorldSurface,
                            x,
                            z,
                            level.section.min_y,
                        ),
                    };
                    for (stats, tile) in results.iter_mut().zip(tiles.iter()) {
                        stats.add(
                            interpolated_height(tile, x as u8, z as u8),
                            actual,
                            cx,
                            cz,
                            x as u8,
                            z as u8,
                        );
                    }
                }
            }
            if (i + 1) % 4 == 0 {
                println!(
                    "seed={seed} {stage:?}: {}/{} chunks in {:.2}s",
                    i + 1,
                    limit,
                    started.elapsed().as_secs_f64()
                );
            }
        }
        println!(
            "\nseed={seed} stage={stage:?} chunks={limit} elapsed={:.2}s",
            started.elapsed().as_secs_f64()
        );
        for (step, stats) in [4, 8, 16].into_iter().zip(results.iter_mut()) {
            stats.print(&format!("step={step}"));
        }
    }
}
