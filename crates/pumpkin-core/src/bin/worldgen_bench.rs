//! Generates a fixed square of chunks through the real chunk system, the
//! way the server's background generation does, then times it and hashes
//! every chunk's saved bytes. Placement checks go through the server's own
//! block registry, as they do in game.
//!
//! Run it from the workspace root:
//!
//! ```text
//! cargo run --profile iterate -p pumpkin -p pumpkin-core --bin worldgen_bench
//! ```
//!
//! With no golden file it writes one. With one, it compares every chunk
//! against it and exits with an error on any difference, so a change to
//! world generation either keeps the output byte for byte or says which
//! chunks moved. `--bless` rewrites the golden file.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use pumpkin_core::block::registry::BlockRegistry;
use pumpkin_core::world::{World, WorldPortal};
use pumpkin_config::world::LevelConfig;
use pumpkin_data::dimension::Dimension;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_world::chunk::ChunkData;
use pumpkin_world::chunk::format::LightContainer;
use pumpkin_util::world_seed::Seed;
use pumpkin_world::level::Level;
use pumpkin_world::world::WorldPortalExt;
use pumpkin_world::world_info::LevelData;

/// The seed of shah's `New World`.
const SEED: i64 = 3_676_290_170_339_937_829;
/// The disc centre of `New World` at the time of writing.
const CENTRE: (i32, i32) = (17, 25);
/// Chunks per side of the square.
const SIDE: i32 = 32;
/// Tickets in flight, as `MAX_TICKETS` in pumpkin-core's lod service.
const IN_FLIGHT: usize = 128;
/// What each column of the golden file hashes, after the chunk position.
const PARTS: [&str; 6] = ["blocks", "biomes", "light", "heightmaps", "ticks", "block_entities"];

/// One hash per part of the chunk, each independent of hash map order.
fn part_hashes(chunk: &ChunkData) -> [u64; 6] {
    use xxhash_rust::xxh64::Xxh64;
    let mut parts = [(); 6].map(|()| Xxh64::new(0));
    let blocks = chunk.section.block_sections.read().unwrap();
    let biomes = chunk.section.biome_sections.read().unwrap();
    for section in blocks.iter() {
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    parts[0].update(&section.get(x, y, z).as_u16().to_le_bytes());
                }
            }
        }
    }
    for section in biomes.iter() {
        for y in 0..4 {
            for z in 0..4 {
                for x in 0..4 {
                    parts[1].update(&[section.get(x, y, z)]);
                }
            }
        }
    }
    let light = chunk.light_engine.lock().unwrap();
    for container in light.sky_light.iter().chain(light.block_light.iter()) {
        match container {
            LightContainer::Empty(level) => parts[2].update(&[0, *level]),
            LightContainer::Full(data) => {
                parts[2].update(&[1]);
                parts[2].update(data);
            }
        }
    }
    let heightmap = chunk.heightmap.lock().unwrap();
    for map in [&heightmap.world_surface, &heightmap.motion_blocking, &heightmap.motion_blocking_no_leaves] {
        for value in map.as_deref().unwrap_or(&[]) {
            parts[3].update(&value.to_le_bytes());
        }
    }
    let mut ticks: Vec<_> = chunk
        .block_ticks
        .to_vec()
        .into_iter()
        .map(|tick| (0u8, tick.position.0.x, tick.position.0.y, tick.position.0.z, tick.delay, tick.priority as i32, tick.value.id.as_u16()))
        .chain(chunk.fluid_ticks.to_vec().into_iter().map(|tick| {
            (1u8, tick.position.0.x, tick.position.0.y, tick.position.0.z, tick.delay, tick.priority as i32, tick.value.id)
        }))
        .collect();
    ticks.sort_unstable();
    parts[4].update(format!("{ticks:?}").as_bytes());
    let mut entities: Vec<_> = chunk
        .pending_block_entities
        .lock()
        .unwrap()
        .iter()
        .map(|(pos, nbt)| {
            let bytes = pumpkin_nbt::Nbt::new(String::new(), nbt.clone()).write_unnamed();
            ((pos.0.x, pos.0.y, pos.0.z), bytes.to_vec())
        })
        .collect();
    entities.sort_unstable();
    for (pos, bytes) in &entities {
        parts[5].update(format!("{pos:?}").as_bytes());
        parts[5].update(bytes);
    }
    parts.map(|part| part.digest())
}

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("worldgen.golden")
}

#[tokio::main]
async fn main() {
    let bless = std::env::args().any(|arg| arg == "--bless");
    let root = std::env::temp_dir().join(format!("worldgen-bench-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create the scratch world");

    let level = Level::from_root_folder(
        &LevelConfig::default(),
        root.clone(),
        SEED,
        Dimension::OVERWORLD,
    );
    // As the server does for each dimension: generation waits for a portal.
    let world = Arc::new(World::load(
        level.clone(),
        Arc::new(ArcSwap::new(Arc::new(LevelData::default(Seed(SEED as u64))))),
        Dimension::OVERWORLD,
        Arc::new(BlockRegistry::default()),
        Weak::new(),
    ));
    let portal: Arc<dyn WorldPortalExt> = Arc::new(WorldPortal(world.clone()));
    level.world_portal.store(Arc::new(Some(portal)));
    let completed = level.chunk_listener.add_global_chunk_listener();

    // Nearest first, as the lod service orders its disc.
    let half = SIDE / 2;
    let mut queue: Vec<Vector2<i32>> = (0..SIDE)
        .flat_map(|x| (0..SIDE).map(move |z| Vector2::new(CENTRE.0 - half + x, CENTRE.1 - half + z)))
        .collect();
    queue.sort_by_key(|pos| {
        let (dx, dz) = (pos.x - CENTRE.0, pos.y - CENTRE.1);
        (dx * dx + dz * dz, pos.x, pos.y)
    });
    let wanted: std::collections::HashSet<_> = queue.iter().copied().collect();
    let mut queue = queue.into_iter();

    let start = Instant::now();
    let mut in_flight = 0;
    let mut done = 0;
    while done < wanted.len() {
        let mut issued = false;
        {
            let mut loading = level
                .chunk_loading
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while in_flight < IN_FLIGHT {
                let Some(pos) = queue.next() else { break };
                loading.add_background_ticket(pos);
                in_flight += 1;
                issued = true;
            }
            if issued {
                loading.send_change();
            }
        }
        match completed.recv_timeout(Duration::from_secs(120)) {
            Ok((pos, _)) if wanted.contains(&pos) => {
                in_flight -= 1;
                done += 1;
            }
            Ok(_) => {}
            Err(error) => panic!("no chunk finished in 120 s ({done} of {} done): {error}", wanted.len()),
        }
    }
    let elapsed = start.elapsed();
    let stages = [
        "None", "Empty", "Biomes", "StructureStart", "StructureReferences", "Noise",
        "Surface", "Carvers", "Features", "Lighting", "Spawn", "Full",
    ];
    let nanos: Vec<u64> = pumpkin_world::chunk_system::STAGE_NANOS
        .iter()
        .map(|n| n.load(std::sync::atomic::Ordering::Relaxed))
        .collect();
    let total: u64 = nanos.iter().sum();
    for (index, name) in stages.iter().enumerate() {
        let runs = pumpkin_world::chunk_system::STAGE_RUNS[index].load(std::sync::atomic::Ordering::Relaxed);
        if runs > 0 {
            println!(
                "{name:>20}: {:7.2} s worker time, {:5.1}%, {runs} runs, {:.2} ms each, longest {:.1} ms",
                nanos[index] as f64 / 1e9,
                100.0 * nanos[index] as f64 / total as f64,
                nanos[index] as f64 / 1e6 / runs as f64,
                pumpkin_world::chunk_system::STAGE_MAX_NANOS[index].load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6
            );
        }
    }

    // Hash once everything is in, so light that crosses into a chunk after
    // it finished is counted the same way every run.
    let mut hashes = BTreeMap::new();
    for &pos in &wanted {
        let chunk = level
            .loaded_chunks
            .get(&pos)
            .unwrap_or_else(|| panic!("chunk {pos:?} finished but is not loaded"));
        hashes.insert((pos.x, pos.y), part_hashes(&chunk));
    }
    let mut table = String::new();
    for ((x, z), parts) in &hashes {
        write!(table, "{x} {z}").unwrap();
        for part in parts {
            write!(table, " {part:016x}").unwrap();
        }
        table.push('\n');
    }
    let digest = xxhash_rust::xxh64::xxh64(table.as_bytes(), 0);

    level.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);

    println!(
        "{} chunks in {:.2} s: {:.1} chunks/s, digest {digest:016x}",
        hashes.len(),
        elapsed.as_secs_f64(),
        hashes.len() as f64 / elapsed.as_secs_f64()
    );

    let golden = golden_path();
    match std::fs::read_to_string(&golden) {
        Ok(expected) if !bless => {
            if expected.lines().count() != table.lines().count() {
                eprintln!("{} has a different chunk set; rerun with --bless", golden.display());
                std::process::exit(1);
            }
            let mut differing = [0usize; 6];
            let mut first = Vec::new();
            for (expected, actual) in expected.lines().zip(table.lines()) {
                let expected: Vec<_> = expected.split(' ').collect();
                let actual: Vec<_> = actual.split(' ').collect();
                for part in 0..PARTS.len() {
                    if expected[2 + part] != actual[2 + part] {
                        differing[part] += 1;
                        if first.len() < 8 {
                            first.push(format!("{} {} {}", actual[0], actual[1], PARTS[part]));
                        }
                    }
                }
            }
            if differing.iter().all(|&count| count == 0) {
                println!("matches {}", golden.display());
            } else {
                for (part, count) in PARTS.iter().zip(differing) {
                    if count > 0 {
                        eprintln!("{part}: {count} of {} chunks differ", hashes.len());
                    }
                }
                eprintln!("first: {first:?}");
                std::process::exit(1);
            }
        }
        _ => {
            std::fs::write(&golden, &table).expect("write golden file");
            println!("wrote {}", golden.display());
        }
    }
}
