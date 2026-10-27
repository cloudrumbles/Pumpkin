#[allow(clippy::wildcard_imports)]
use super::*;

impl PendingConnection {
    pub async fn handle_known_packs(&mut self, server: &Server) {
        let version = CURRENT_MC_VERSION;
        let registry = pumpkin_data::registry::Registry::get_synced(version);
        for reg in &registry {
            if reg.registry_id == "minecraft:damage_type" {
                let merged = server
                    .datapack_manager
                    .merge_damage_type_entries(&reg.registry_entries);
                self.send_packet_now(&CRegistryData::new(&reg.registry_id, &merged))
                    .await;
                continue;
            }
            let mut merged = server
                .datapack_manager
                .merge_registry_entries(&reg.registry_id, &reg.registry_entries);
            if reg.registry_id == "minecraft:dimension_type"
                && let Some(height) = server.basic_config.terrain.overworld_height()
            {
                set_overworld_height(&mut merged, height);
            }
            self.send_packet_now(&CRegistryData::new(&reg.registry_id, &merged))
                .await;
        }
        let mut tags = Vec::new();
        for &key in pumpkin_data::tag::RegistryKey::NETWORK_KEYS {
            if pumpkin_data::tag::get_registry_key_tags(version, key)
                .is_some_and(|map| !map.is_empty())
            {
                tags.push(key);
            }
        }
        self.send_packet_now(&CUpdateTags::new(&tags)).await;
        self.send_packet_now(&CFinishConfig).await;
    }
}

/// Gives the overworld's dimension type the height the server built the
/// overworld with, so the client sizes its chunks the same way.
fn set_overworld_height(entries: &mut [pumpkin_data::registry::RegistryEntryData], height: i32) {
    use pumpkin_nbt::{Nbt, deserializer::NbtReadHelperJava};
    for entry in entries {
        if entry.entry_id.to_string() != "minecraft:overworld" {
            continue;
        }
        let Some(data) = &entry.data else { continue };
        let mut cursor = std::io::Cursor::new(&data[..]);
        let mut nbt = Nbt::read_unnamed(&mut NbtReadHelperJava::new(&mut cursor))
            .expect("the built-in overworld dimension type is valid NBT");
        nbt.root_tag.put_int("height", height);
        nbt.root_tag.put_int("logical_height", height);
        entry.data = Some(nbt.write_unnamed().to_vec().into_boxed_slice());
    }
}
