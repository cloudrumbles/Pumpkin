use std::sync::Arc;

use pumpkin_command::argument_types::block::BlockStateArgument;
use pumpkin_command::argument_types::block_predicate::BlockPredicate;
use pumpkin_data::{Block, BlockDirection, BlockStateId};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

use crate::block::entities::{block_entity_from_nbt, create_block_entity};
use crate::world::World;

pub(super) fn placement_flags(strict: bool) -> BlockFlags {
    let flags = BlockFlags::NOTIFY_LISTENERS | BlockFlags::SKIP_BLOCK_ENTITY_REPLACED_CALLBACK;
    if strict {
        flags
            | BlockFlags::FORCE_STATE
            | BlockFlags::SKIP_DROPS
            | BlockFlags::SKIP_BLOCK_ADDED_CALLBACK
            | BlockFlags::SKIP_NEIGHBOR_SHAPE_UPDATE
    } else {
        flags
    }
}

fn placement_state(
    world: &World,
    position: &BlockPos,
    input: &BlockStateArgument,
    flags: BlockFlags,
) -> BlockStateId {
    if flags.contains(BlockFlags::SKIP_NEIGHBOR_SHAPE_UPDATE) {
        return input.state.id;
    }
    let mut state = input.state.id;
    for direction in BlockDirection::abstract_block_update_order() {
        let neighbor = position.offset(direction.to_offset());
        state = world.block_registry.get_state_for_neighbor_update(
            world,
            Block::from_state_id(state),
            state,
            position,
            direction,
            &neighbor,
            world.get_block_state_id(&neighbor),
        );
    }
    if state.to_state().is_air() {
        state = input.state.id;
    }
    let block = Block::from_state_id(state);
    if let Some(properties) = block.properties(state) {
        let mut properties = properties.to_props();
        for (name, value) in &input.properties {
            if let Some((_, actual)) = properties.iter_mut().find(|(key, _)| *key == name) {
                *actual = value;
            }
        }
        state = block
            .state_from_properties(&properties)
            .map_or(state, |state| state.id);
    }
    state
}

pub(super) fn notify_neighbors(world: &Arc<World>, position: &BlockPos, previous: BlockStateId) {
    world.update_neighbors_at(position, Block::from_state_id(previous), None);
    let state = world.get_block_state_id(position);
    if state.has_analog_output_signal() {
        world.update_neighbour_for_output_signal(position, Block::from_state_id(state));
    }
}

pub(super) fn matches_block(
    world: &World,
    position: &BlockPos,
    predicate: &BlockPredicate,
) -> bool {
    let nbt = predicate
        .requires_nbt()
        .then(|| {
            world.get_block_entity(position).map(|entity| {
                let mut nbt = NbtCompound::new();
                entity.write_internal(&mut nbt);
                nbt
            })
        })
        .flatten();
    predicate.test(world.get_block_state(position), nbt.as_ref())
}

/// Set a command's state and load its block entity data, counting NBT-only changes.
pub(super) fn place_block(
    world: &Arc<World>,
    position: &BlockPos,
    input: &BlockStateArgument,
    flags: BlockFlags,
) -> bool {
    if !world.is_in_build_limit(*position) || !world.is_loaded(position) {
        return false;
    }
    let state_id = placement_state(world, position, input, flags);
    let previous = world.set_block_state(position, state_id, flags);
    let changed = previous != state_id;
    let state = world.get_block_state(position);
    if Block::from_state_id(state.id) != input.block {
        return changed;
    }

    // Strict placement skips on_placed callbacks, but still creates block entities.
    let entity = world.get_block_entity(position).or_else(|| {
        let entity = create_block_entity(state.block_entity_type, *position)?;
        world.add_block_entity(entity.clone());
        Some(entity)
    });
    let (Some(entity), Some(tag)) = (entity, &input.nbt) else {
        return changed;
    };
    let mut before = NbtCompound::new();
    entity.write_internal(&mut before);
    let mut loaded = tag.clone();
    // Loading command data cannot change a block entity's type or position.
    loaded.put_string("id", entity.resource_location().to_string());
    loaded.put_int("x", position.0.x);
    loaded.put_int("y", position.0.y);
    loaded.put_int("z", position.0.z);
    let Some(replacement) = block_entity_from_nbt(&loaded) else {
        return changed;
    };
    let mut after = NbtCompound::new();
    replacement.write_internal(&mut after);
    if before == after {
        return changed;
    }
    world.add_block_entity_nbt(*position, &after);
    world.add_block_entity(replacement);
    true
}

#[cfg(test)]
mod tests {
    use super::{matches_block, place_block, placement_flags};
    use crate::world::tests::TestWorld;
    use pumpkin_command::argument_types::argument_type::ArgumentType;
    use pumpkin_command::argument_types::block::{BlockStateArgument, BlockStateArgumentType};
    use pumpkin_command::argument_types::block_predicate::BlockPredicateArgumentType;
    use pumpkin_command::source::DummySource;
    use pumpkin_command::string_reader::StringReader;
    use pumpkin_nbt::compound::NbtCompound;
    use pumpkin_util::math::position::BlockPos;
    use pumpkin_world::chunk::{ChunkData, io::Dirtiable};

    fn input(value: &str) -> BlockStateArgument {
        ArgumentType::<DummySource>::parse(&BlockStateArgumentType, &mut StringReader::new(value))
            .expect("block input")
    }

    fn fixture(position: BlockPos) -> TestWorld {
        let fixture = TestWorld::new();
        let chunk_pos = position.chunk_position();
        fixture
            .level
            .loaded_chunks
            .insert(chunk_pos, ChunkData::empty_sync(chunk_pos.x, chunk_pos.y));
        fixture
    }

    #[tokio::test]
    async fn counts_nbt_only_changes_and_rejects_unchanged_input() {
        let position = BlockPos::new(5, 100, 7);
        let fixture = fixture(position);
        let empty = input("chest");
        assert!(place_block(
            &fixture.world,
            &position,
            &empty,
            placement_flags(false)
        ));
        assert!(!place_block(
            &fixture.world,
            &position,
            &empty,
            placement_flags(false)
        ));
        let filled = input(r#"chest{Items:[{Slot:0b,id:"minecraft:diamond",count:3}]}"#);
        assert!(place_block(
            &fixture.world,
            &position,
            &filled,
            placement_flags(false)
        ));
        assert!(!place_block(
            &fixture.world,
            &position,
            &filled,
            placement_flags(false)
        ));
        let cleared = input("chest{Items:[]}");
        assert!(place_block(
            &fixture.world,
            &position,
            &cleared,
            placement_flags(false)
        ));
        fixture.shutdown().await;
    }

    #[tokio::test]
    async fn strict_placement_keeps_block_entity_identity_and_saved_data() {
        let position = BlockPos::new(5, 100, 7);
        let fixture = fixture(position);
        let block = input(
            r#"chest{id:"minecraft:furnace",x:1,y:2,z:3,Items:[{Slot:0b,id:"minecraft:diamond",count:3}]}"#,
        );
        assert!(place_block(
            &fixture.world,
            &position,
            &block,
            placement_flags(true)
        ));
        let predicate = ArgumentType::<DummySource>::parse(
            &BlockPredicateArgumentType,
            &mut StringReader::new(r#"chest{id:"minecraft:chest",x:5,y:100,z:7,Items:[{id:"minecraft:diamond",count:3}]}"#),
        ).expect("predicate");
        assert!(matches_block(&fixture.world, &position, &predicate));
        let mut actual = NbtCompound::new();
        fixture
            .world
            .get_block_entity(&position)
            .expect("live block entity")
            .write_internal(&mut actual);
        let chunk = fixture
            .level
            .loaded_chunks
            .get(&position.chunk_position())
            .unwrap()
            .clone();
        assert!(chunk.is_dirty());
        assert_eq!(
            chunk.pending_block_entities.lock().unwrap().get(&position),
            Some(&actual)
        );
        fixture.shutdown().await;
    }

    #[tokio::test]
    async fn replacing_a_container_removes_its_live_and_saved_nbt() {
        let position = BlockPos::new(5, 100, 7);
        let fixture = fixture(position);
        let block = input(r#"chest{Items:[{Slot:0b,id:"minecraft:diamond",count:3}]}"#);
        assert!(place_block(
            &fixture.world,
            &position,
            &block,
            placement_flags(false)
        ));
        assert!(place_block(
            &fixture.world,
            &position,
            &input("stone"),
            placement_flags(false)
        ));
        assert!(fixture.world.get_block_entity(&position).is_none());
        let chunk = fixture
            .level
            .loaded_chunks
            .get(&position.chunk_position())
            .unwrap()
            .clone();
        assert!(
            !chunk
                .pending_block_entities
                .lock()
                .unwrap()
                .contains_key(&position)
        );
        fixture.shutdown().await;
    }
}
