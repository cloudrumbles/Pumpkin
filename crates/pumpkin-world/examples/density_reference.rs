//! Compares bounded top-of-density search to an exhaustive vertical search.
//! Run in release-like profile; never substitutes for full-block visual parity.
#![allow(clippy::print_stdout, clippy::expect_used, clippy::unwrap_used)]
use pumpkin_data::dimension::Dimension;
use pumpkin_util::world_seed::Seed;
use pumpkin_world::generation::density_surface::DensitySurfaceSampler;
use pumpkin_world::generation::generator::VanillaGenerator;
use std::time::Instant;

fn main() {
    let n = std::env::args()
        .nth(1)
        .map_or(1024, |v| v.parse::<usize>().expect("sample count"));
    assert!(n > 0 && n <= 100_000);
    for seed in [42, 3_676_290_170_339_937_829] {
        let generator = VanillaGenerator::new_with_settings(Seed(seed), Dimension::OVERWORLD, None);
        let mut bound = DensitySurfaceSampler::new(&generator, 8);
        let mut exhaustive = DensitySurfaceSampler::new(&generator, 8);
        let start = Instant::now();
        let mut wrong = 0usize;
        let mut total_difference = 0u64;
        let mut greatest = (0i32, 0i32, 0i32, 0i32);
        for i in 0..n {
            // Deterministic pseudorandom world positions, spread over the 512-chunk LOD disc.
            let x = ((i as u32).wrapping_mul(0x9e37_79b9) % 16_384) as i32 - 8192;
            let z = ((i as u32).wrapping_mul(0x85eb_ca6b) % 16_384) as i32 - 8192;
            let guess = bound.estimate_height(x, z);
            let exact = exhaustive.estimate_height_exhaustive(x, z);
            let difference = guess.abs_diff(exact);
            if difference > 0 {
                wrong += 1;
                total_difference += u64::from(difference);
                if difference > greatest.2.abs_diff(greatest.3) {
                    greatest = (x, z, guess, exact);
                }
            }
        }
        println!(
            "seed {seed}: {n} coordinate pairs checked in {:.2}s; clipped {wrong} ({:.2}%), cumulative height loss {total_difference}; worst position/guess/reference={greatest:?}",
            start.elapsed().as_secs_f64(),
            wrong as f64 * 100.0 / n as f64
        );
    }
}
