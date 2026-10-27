//! Reproducible isolated benchmark for provisional native-noise height tiles.
//! Run: `cargo run --profile iterate -p pumpkin-world --example coarse_surface -- 8192`
//! It does not start a server, write chunks, or claim canonical terrain.

#![allow(clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

use std::hint::black_box;
use std::io::Write as _;
use std::time::Instant;

use pumpkin_data::dimension::Dimension;
use pumpkin_util::world_seed::Seed;
use pumpkin_world::generation::density_surface::DensitySurfaceSampler;
use pumpkin_world::generation::generator::VanillaGenerator;
use rayon::prelude::*;

fn main() {
    let count: usize = std::env::args()
        .nth(1)
        .map_or(8192, |n| n.parse().expect("number of columns"));
    assert!(count > 0 && count <= 100_000);
    let generator = VanillaGenerator::new_with_settings(Seed(42), Dimension::OVERWORLD, None);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    println!("native final-density surface sampler, {count} columns, 4 threads");
    // Consume every tile, including its compact wire representation, so the
    // compiler cannot optimise the generator into a constant.
    for step in [16, 8, 4] {
        let started = Instant::now();
        let (checksum, total_wire_bytes, low, high) = pool.install(|| {
            (0..count)
                .into_par_iter()
                .map_init(
                    || DensitySurfaceSampler::new(&generator, 8),
                    |sampler, index| {
                        let x = (index % 128) as i32 - 64;
                        let z = (index / 128) as i32 - 64;
                        let tile = sampler.sample_tile(x, z, step).expect("valid grid");
                        let bytes = tile.encode();
                        let sum: u64 = tile
                            .heights
                            .iter()
                            .map(|height| (*height as i64 as u64).rotate_left(9))
                            .fold(0, u64::wrapping_add);
                        (
                            black_box(sum),
                            black_box(bytes.len()),
                            *tile.heights.iter().min().unwrap(),
                            *tile.heights.iter().max().unwrap(),
                        )
                    },
                )
                .reduce(
                    || (0u64, 0usize, i16::MAX, i16::MIN),
                    |a, b| (a.0.wrapping_add(b.0), a.1 + b.1, a.2.min(b.2), a.3.max(b.3)),
                )
        });
        let elapsed = started.elapsed().as_secs_f64();
        println!(
            "step={step}: {count} tiles, {elapsed:.3}s, {:.0} tiles/s, {:.0} KiB/s, heights=[{low},{high}], checksum={checksum:016x}",
            count as f64 / elapsed,
            total_wire_bytes as f64 / elapsed / 1024.0,
        );
    }
    if let Some(path) = std::env::var_os("IRONORE_HEIGHT_TILE_DUMP") {
        let path = std::path::PathBuf::from(path);
        write_sample_frames(&generator, count, &path).expect("write sample frames");
        println!("wrote {count} native-noise tiles to {}", path.display());
    }
}

/// Emits actual Pumpkin-generated prototype frames for the separate Ironore
/// decoder example. Only run when an explicit output path is provided.
fn write_sample_frames(
    generator: &VanillaGenerator,
    count: usize,
    path: &std::path::Path,
) -> std::io::Result<()> {
    let mut writer = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut sampler = DensitySurfaceSampler::new(generator, 8);
    for index in 0..count {
        let x = (index % 128) as i32 - 64;
        let z = (index / 128) as i32 - 64;
        let tile = sampler.sample_tile(x, z, 4).expect("valid grid");
        let wire = tile.encode();
        writer.write_all(&(wire.len() as u16).to_be_bytes())?;
        writer.write_all(&wire)?;
    }
    writer.flush()
}
