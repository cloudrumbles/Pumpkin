//! Writes the blocks around the surface of a square of saved chunks, by
//! name, so ironore can compare far terrain built from a terrain-diffusion
//! heightmap with the real chunks there.
//!
//! ```text
//! chunk_band_dump <world folder> <chunk x> <chunk z> <chunks per side> <out.json>
//! ```
//!
//! Each column runs from 24 blocks under its highest block to 16 above it.

#![allow(clippy::print_stdout)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use pumpkin_config::world::LevelConfig;
use pumpkin_data::BlockState;
use pumpkin_data::dimension::Dimension;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_world::chunk::io::{FileIO, LoadedData};
use pumpkin_world::level::Level;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, world, cx, cz, side, out] = &args[..] else {
        panic!("usage: chunk_band_dump <world folder> <chunk x> <chunk z> <chunks per side> <out.json>");
    };
    let (cx, cz, side): (i32, i32, i32) = (cx.parse().unwrap(), cz.parse().unwrap(), side.parse().unwrap());
    let dimension = Dimension {
        height: 2096,
        logical_height: 2096,
        ..Dimension::OVERWORLD
    };
    let level = Level::from_root_folder(&LevelConfig::default(), PathBuf::from(world), 0, dimension);
    let coords: Vec<_> = (cx..cx + side)
        .flat_map(|x| (cz..cz + side).map(move |z| Vector2::new(x, z)))
        .collect();
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let saver = Arc::clone(&level.chunk_saver);
    let folder = Arc::clone(&level.level_folder);
    let fetch = tokio::spawn(async move { saver.fetch_chunks(&folder, &coords, tx).await });
    let mut names: Vec<&'static str> = Vec::new();
    let mut ids: HashMap<&'static str, usize> = HashMap::new();
    let mut columns = Vec::new();
    let (mut missing, mut failed) = (0, 0);
    while let Some(loaded) = rx.recv().await {
        let chunk = match loaded {
            LoadedData::Loaded(chunk) => chunk,
            LoadedData::Missing(_) => {
                missing += 1;
                continue;
            }
            LoadedData::Error(_) => {
                failed += 1;
                continue;
            }
        };
        let min_y = chunk.section.min_y;
        let max_y = min_y + chunk.section.count as i32 * 16;
        for z in 0..16 {
            for x in 0..16 {
                let name_at = |y| {
                    chunk
                        .section
                        .get_block_absolute_y(x, y, z)
                        .map_or("air", |id| BlockState::from_id(id).id.to_block().name)
                };
                let Some(top) = (min_y..max_y).rev().find(|&y| name_at(y) != "air") else {
                    continue;
                };
                let y0 = (top - 24).max(min_y);
                let band: Vec<usize> = (y0..(top + 17).min(max_y))
                    .map(|y| {
                        let name = name_at(y);
                        *ids.entry(name).or_insert_with(|| {
                            names.push(name);
                            names.len() - 1
                        })
                    })
                    .collect();
                columns.push(serde_json::json!({
                    "x": chunk.x * 16 + x as i32,
                    "z": chunk.z * 16 + z as i32,
                    "y0": y0,
                    "b": band,
                }));
            }
        }
    }
    fetch.await.unwrap();
    let count = columns.len();
    std::fs::write(out, serde_json::json!({ "names": names, "columns": columns }).to_string()).unwrap();
    println!("{count} columns written, {missing} chunks missing, {failed} unreadable");
    level.shutdown().await;
}
