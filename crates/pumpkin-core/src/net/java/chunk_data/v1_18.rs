use super::util::write_compound_nbt;
use pumpkin_protocol::codec::bit_set::BitSet;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::ser::NetworkWriteExt;
use pumpkin_protocol::ser::WritingError;
use pumpkin_util::math::position::get_local_cord;
use pumpkin_util::version::JavaMinecraftVersion;
use pumpkin_world::chunk::ChunkData;
use pumpkin_world::chunk::format::LightContainer;
use pumpkin_world::chunk::palette::NetworkPalette;
use std::io::Write;

/// Serializes chunk data for Minecraft 1.18+ through 26.2+.
#[expect(clippy::too_many_lines)]
pub fn write_chunk_data(
    chunk: &ChunkData,
    mut write: impl Write,
    version: &JavaMinecraftVersion,
) -> Result<(), WritingError> {
    write.write_i32_be(chunk.x)?;
    write.write_i32_be(chunk.z)?;

    let heightmaps = chunk
        .heightmap
        .lock()
        .map_err(|_| WritingError::Message("heightmap lock poisoned".into()))?;
    if version >= &JavaMinecraftVersion::V_1_21_5 {
        write.write_var_int(&VarInt(3))?; // Map size

        let mut write_heightmap = |index: i32, data: &[i64]| -> Result<(), WritingError> {
            write.write_var_int(&VarInt(index))?;
            write.write_var_int(&VarInt(data.len() as i32))?;
            for val in data {
                write.write_i64_be(*val)?;
            }
            Ok(())
        };

        let empty = vec![0; pumpkin_world::chunk::ChunkHeightmaps::longs(heightmaps.bits)];
        write_heightmap(1, heightmaps.world_surface.as_deref().unwrap_or(&empty))?;
        write_heightmap(4, heightmaps.motion_blocking.as_deref().unwrap_or(&empty))?;
        write_heightmap(
            5,
            heightmaps
                .motion_blocking_no_leaves
                .as_deref()
                .unwrap_or(&empty),
        )?;
    } else {
        let mut comp = pumpkin_nbt::compound::NbtCompound::new();
        if let Some(ref ws) = heightmaps.world_surface {
            comp.put(
                "WORLD_SURFACE",
                pumpkin_nbt::tag::NbtTag::LongArray(ws.to_vec()),
            );
        }
        if let Some(ref mb) = heightmaps.motion_blocking {
            comp.put(
                "MOTION_BLOCKING",
                pumpkin_nbt::tag::NbtTag::LongArray(mb.to_vec()),
            );
        }
        if let Some(ref mbnl) = heightmaps.motion_blocking_no_leaves {
            comp.put(
                "MOTION_BLOCKING_NO_LEAVES",
                pumpkin_nbt::tag::NbtTag::LongArray(mbnl.to_vec()),
            );
        }
        write_compound_nbt(&mut write, &comp, *version)?;
    }
    drop(heightmaps);

    {
        let mut blocks_and_biomes_buf = Vec::new();
        let block_sections = chunk
            .section
            .block_sections
            .read()
            .map_err(|_| WritingError::Message("block_sections read lock poisoned".into()))?;
        let biome_sections = chunk
            .section
            .biome_sections
            .read()
            .map_err(|_| WritingError::Message("biome_sections read lock poisoned".into()))?;

        let mut zero_bytes_count = 0;

        for (block_palette, biome_palette) in block_sections.iter().zip(biome_sections.iter()) {
            let non_empty_block_count = block_palette.non_air_block_count() as i16;
            blocks_and_biomes_buf.write_i16_be(non_empty_block_count)?;
            if version >= &JavaMinecraftVersion::V_26_1 {
                // New in 26.1, fluid count
                let liquid_count = block_palette.liquid_block_count() as i16;
                blocks_and_biomes_buf.write_i16_be(liquid_count)?;
            }

            let block_network = block_palette.convert_network();
            blocks_and_biomes_buf.write_u8(block_network.bits_per_entry)?;

            match block_network.palette {
                NetworkPalette::Single(registry_id) => {
                    blocks_and_biomes_buf.write_var_int(&registry_id.into())?;
                }
                NetworkPalette::Indirect(palette) => {
                    blocks_and_biomes_buf.write_var_int(&palette.len().try_into().map_err(
                        |_| {
                            WritingError::Message(format!(
                                "{} is not representable as a VarInt!",
                                palette.len()
                            ))
                        },
                    )?)?;
                    for registry_id in palette {
                        blocks_and_biomes_buf.write_var_int(&registry_id.into())?;
                    }
                }
                NetworkPalette::Direct => {}
            }

            if version <= &JavaMinecraftVersion::V_1_21_4 {
                blocks_and_biomes_buf.write_list(&block_network.packed_data, |buf, &packed| {
                    buf.write_i64_be(packed)
                })?;
            } else {
                for packed in &block_network.packed_data {
                    blocks_and_biomes_buf.write_i64_be(*packed)?;
                }
            }

            let biome_network = biome_palette.convert_network();
            blocks_and_biomes_buf.write_u8(biome_network.bits_per_entry)?;

            match biome_network.palette {
                NetworkPalette::Single(registry_id) => {
                    blocks_and_biomes_buf.write_var_int(&registry_id.into())?;
                }
                NetworkPalette::Indirect(palette) => {
                    blocks_and_biomes_buf.write_var_int(&palette.len().try_into().map_err(
                        |_| {
                            WritingError::Message(format!(
                                "{} is not representable as a VarInt!",
                                palette.len()
                            ))
                        },
                    )?)?;
                    for registry_id in palette {
                        blocks_and_biomes_buf.write_var_int(&registry_id.into())?;
                    }
                }
                NetworkPalette::Direct => {}
            }

            if version <= &JavaMinecraftVersion::V_1_21_4 {
                blocks_and_biomes_buf.write_list(&biome_network.packed_data, |buf, &packed| {
                    buf.write_i64_be(packed)
                })?;
            } else {
                for packed in &biome_network.packed_data {
                    blocks_and_biomes_buf.write_i64_be(*packed)?;
                }
            }

            if version == &JavaMinecraftVersion::V_1_21_5 {
                let block_storage_len = block_network.packed_data.len() as i32;
                let biome_storage_len = biome_network.packed_data.len() as i32;
                zero_bytes_count += VarInt(block_storage_len).written_size()
                    + VarInt(biome_storage_len).written_size();
            }
        }

        if version == &JavaMinecraftVersion::V_1_21_5 && zero_bytes_count > 0 {
            blocks_and_biomes_buf.resize(blocks_and_biomes_buf.len() + zero_bytes_count, 0);
        }

        write.write_var_int(&blocks_and_biomes_buf.len().try_into().map_err(|_| {
            WritingError::Message(format!(
                "{} is not representable as a VarInt!",
                blocks_and_biomes_buf.len()
            ))
        })?)?;
        write.write_slice(&blocks_and_biomes_buf)?;
    };

    let block_entities = chunk
        .pending_block_entities
        .lock()
        .map_err(|_| WritingError::Message("block_entities lock poisoned".into()))?;
    write.write_var_int(&VarInt(block_entities.len() as i32))?;
    for (pos, nbt) in block_entities.iter() {
        let local_xz = ((get_local_cord(pos.0.x) & 0xF) << 4) | (get_local_cord(pos.0.z) & 0xF);

        write.write_u8(local_xz as u8)?;
        write.write_i16_be(pos.0.y as i16)?;

        let id = nbt.get_string("id").map_or(0, |id_str| {
            let name = id_str.split(':').next_back().unwrap_or(id_str);
            pumpkin_data::block_properties::BLOCK_ENTITY_TYPES
                .iter()
                .position(|&n| n == name)
                .unwrap_or(0)
        });
        write.write_var_int(&VarInt(id as i32))?;

        let mut client_nbt = nbt.clone();
        client_nbt.child_tags.remove("id");
        client_nbt.child_tags.remove("x");
        client_nbt.child_tags.remove("y");
        client_nbt.child_tags.remove("z");
        client_nbt.child_tags.remove("LootTable");
        client_nbt.child_tags.remove("LootTableSeed");
        client_nbt.child_tags.remove("PumpkinCustomData");
        client_nbt.child_tags.remove("BukkitValues");
        write_compound_nbt(&mut write, &client_nbt, *version)?;
    }

    {
        // Light masks include sections from -1 (below world) to num_sections (above world)
        // This means we need to account for 2 extra sections in the bitset
        let light_engine = chunk
            .light_engine
            .lock()
            .map_err(|_| WritingError::Message("light_engine lock poisoned".into()))?;
        let num_sections = light_engine.sky_light.len();

        // A dimension taller than 62 sections needs several words per mask.
        let words = (num_sections + 2).div_ceil(64);
        let mut sky_light_empty_mask = vec![0i64; words];
        let mut block_light_empty_mask = vec![0i64; words];
        let mut sky_light_mask = vec![0i64; words];
        let mut block_light_mask = vec![0i64; words];
        let set = |mask: &mut [i64], bit: usize| mask[bit / 64] |= 1 << (bit % 64);

        // Bit 0 represents the section below the world (always empty)
        set(&mut sky_light_empty_mask, 0);
        set(&mut block_light_empty_mask, 0);

        // Bits 1..=num_sections represent the actual world sections
        for section_index in 0..num_sections {
            let bit_index = section_index + 1; // Offset by 1 for the below-world section

            if let LightContainer::Full(_) = &light_engine.sky_light[section_index] {
                set(&mut sky_light_mask, bit_index);
            } else {
                set(&mut sky_light_empty_mask, bit_index);
            }

            if let LightContainer::Full(_) = &light_engine.block_light[section_index] {
                set(&mut block_light_mask, bit_index);
            } else {
                set(&mut block_light_empty_mask, bit_index);
            }
        }

        // Bit num_sections+1 represents the section above the world (always empty)
        set(&mut sky_light_empty_mask, num_sections + 1);
        set(&mut block_light_empty_mask, num_sections + 1);

        // Trust edges (1.18 - 1.19.4; removed in 1.20)
        if version < &JavaMinecraftVersion::V_1_20 {
            write.write_bool(true)?;
        }

        // Write Sky Light Mask
        let sky_count: u32 = sky_light_mask.iter().map(|word| word.count_ones()).sum();
        let block_count: u32 = block_light_mask.iter().map(|word| word.count_ones()).sum();
        BitSet(sky_light_mask.into_boxed_slice()).encode_with_version(&mut write, version)?;
        // Write Block Light Mask
        BitSet(block_light_mask.into_boxed_slice()).encode_with_version(&mut write, version)?;
        // Write Empty Sky Light Mask
        BitSet(sky_light_empty_mask.into_boxed_slice()).encode_with_version(&mut write, version)?;
        // Write Empty Block Light Mask
        BitSet(block_light_empty_mask.into_boxed_slice())
            .encode_with_version(&mut write, version)?;

        let light_data_size: VarInt = VarInt(LightContainer::ARRAY_SIZE as i32);

        // Write Sky Light arrays
        write.write_var_int(&VarInt(sky_count as i32))?;
        for section_index in 0..num_sections {
            if let LightContainer::Full(data) = &light_engine.sky_light[section_index] {
                write.write_var_int(&light_data_size)?;
                write.write_slice(data.as_ref())?;
            }
        }

        // Write Block Light arrays
        write.write_var_int(&VarInt(block_count as i32))?;
        for section_index in 0..num_sections {
            if let LightContainer::Full(data) = &light_engine.block_light[section_index] {
                write.write_var_int(&light_data_size)?;
                write.write_slice(data.as_ref())?;
            }
        }
    }

    Ok(())
}
