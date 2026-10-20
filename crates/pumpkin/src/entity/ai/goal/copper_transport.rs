//! Copper golem chest transport, including bounded searches and atomic item transfers.
use crate::{
    block::{
        blocks::chests::is_chest_blocked,
        entities::{BlockEntity, chest::ChestBlockEntity, trapped_chest::TrappedChestBlockEntity},
    },
    entity::{
        ai::{
            goal::{Controls, Goal},
            pathfinder::NavigatorGoal,
        },
        mob::Mob,
        passive::copper_golem::{CopperGolemEntity, CopperGolemState},
    },
    world::World,
};
use pumpkin_data::{
    Block,
    block_properties::{ChestLikeProperties, ChestType},
    data_component_impl::EquipmentSlot,
    item_stack::ItemStack,
    sound::Sound,
    tag::{self, Taggable},
};
use pumpkin_inventory::Inventory;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use std::sync::{Arc, RwLock};

struct ChestTarget {
    position: BlockPos,
    parts: Vec<Arc<dyn BlockEntity>>,
}
impl ChestTarget {
    fn load(world: &World, position: BlockPos) -> Option<Self> {
        let state = world.get_block_state(&position);
        let block = world.get_block(&position);
        if block.id != Block::CHEST.id
            && block.id != Block::TRAPPED_CHEST.id
            && !block.has_tag(&tag::Block::MINECRAFT_COPPER_CHESTS)
        {
            return None;
        }
        if is_chest_blocked(world, &position) {
            return None;
        }
        let mut parts = vec![world.get_block_entity(&position)?];
        let properties = ChestLikeProperties::from_state_id(state.id);
        let connected = match properties.r#type {
            ChestType::Single => None,
            ChestType::Left => Some(properties.facing.rotate_clockwise()),
            ChestType::Right => Some(properties.facing.rotate_counter_clockwise()),
        };
        if let Some(direction) = connected {
            let neighbor = position.offset(direction.to_offset());
            if world.get_block(&neighbor).id != block.id || is_chest_blocked(world, &neighbor) {
                return None;
            }
            parts.push(world.get_block_entity(&neighbor)?);
        }
        // Every golem locks the halves in the same order.
        parts.sort_by_key(|entity| {
            let p = entity.get_position().0;
            (p.x, p.y, p.z)
        });
        if parts
            .iter()
            .any(|entity| Self::storage(entity.as_ref()).is_none())
        {
            return None;
        }
        Some(Self { position, parts })
    }
    fn storage(entity: &dyn BlockEntity) -> Option<&RwLock<[ItemStack; 27]>> {
        if let Some(chest) = entity.as_any().downcast_ref::<ChestBlockEntity>() {
            Some(&chest.items)
        } else {
            entity
                .as_any()
                .downcast_ref::<TrappedChestBlockEntity>()
                .map(|chest| &chest.items)
        }
    }
    fn inventories(&self) -> Vec<Arc<dyn Inventory>> {
        self.parts
            .iter()
            .filter_map(|entity| entity.clone().get_inventory())
            .collect()
    }
    fn unpack_loot(&self) {
        for entity in &self.parts {
            if let Some((key, seed)) = entity.take_loot_table()
                && let Some(table) = pumpkin_data::loot_table::get_loot_table(&key)
                && let Some(inventory) = entity.clone().get_inventory()
            {
                crate::world::loot::fill_chest_inventory(&inventory, table, seed);
                inventory.mark_dirty();
            }
        }
    }
    fn transfer(&self, golem: &CopperGolemEntity, commit: bool) -> CopperGolemState {
        let living = &golem.mob_entity.living_entity;
        let mut equipment = living
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut hand = equipment.get(&EquipmentSlot::MAIN_HAND);
        let mut guards: Vec<_> = self
            .parts
            .iter()
            .filter_map(|entity| Self::storage(entity.as_ref()))
            .map(|items| {
                items
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
            })
            .collect();
        let mut changed = false;
        let state = if hand.is_empty() {
            if let Some(stack) = guards
                .iter_mut()
                .flat_map(|items| items.iter_mut())
                .find(|stack| !stack.is_empty())
            {
                if commit {
                    hand = stack.split(16);
                    changed = true;
                }
                CopperGolemState::GettingItem
            } else {
                CopperGolemState::GettingNoItem
            }
        } else {
            let matching = guards
                .iter()
                .flat_map(|items| items.iter())
                .all(ItemStack::is_empty)
                || guards
                    .iter()
                    .flat_map(|items| items.iter())
                    .any(|stack| !stack.is_empty() && stack.item.id == hand.item.id);
            let mut fits = false;
            if matching {
                for stack in guards.iter_mut().flat_map(|items| items.iter_mut()) {
                    if stack.is_empty() {
                        fits = true;
                        if commit {
                            *stack = hand.clone();
                            hand = ItemStack::EMPTY.clone();
                            changed = true;
                        }
                        break;
                    }
                    if stack.are_items_and_components_equal(&hand)
                        && stack.item_count < stack.get_max_stack_size()
                    {
                        fits = true;
                        if commit {
                            let count = hand
                                .item_count
                                .min(stack.get_max_stack_size() - stack.item_count);
                            stack.item_count += count;
                            hand.item_count -= count;
                            changed |= count > 0;
                            if hand.is_empty() {
                                break;
                            }
                        }
                    }
                }
            }
            if fits {
                CopperGolemState::DroppingItem
            } else {
                CopperGolemState::DroppingNoItem
            }
        };
        if changed {
            equipment.put(&EquipmentSlot::MAIN_HAND, hand.clone());
        }
        drop(guards);
        drop(equipment);
        if changed {
            living
                .equipment_drop_chances
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(EquipmentSlot::MAIN_HAND, 2.0);
            living.send_equipment_changes(&[(EquipmentSlot::MAIN_HAND, hand)]);
            for inventory in self.inventories() {
                inventory.mark_dirty();
            }
        }
        state
    }
}

pub struct CopperTransportGoal {
    target: Option<ChestTarget>,
    opened: Vec<Arc<dyn Inventory>>,
    visited: Vec<(BlockPos, i64)>,
    ticks_at_target: i32,
    path_deadline: i64,
    next_search: i64,
}
impl Default for CopperTransportGoal {
    fn default() -> Self {
        Self {
            target: None,
            opened: Vec::new(),
            visited: Vec::new(),
            ticks_at_target: 0,
            path_deadline: 0,
            next_search: 0,
        }
    }
}
impl CopperTransportGoal {
    fn close(&mut self, golem: &CopperGolemEntity) {
        for inventory in self.opened.drain(..) {
            inventory.on_close();
        }
        golem.opened_chest.store(None);
        golem.set_state(CopperGolemState::Idle);
    }
    fn navigate(&self, mob: &dyn Mob) {
        if let Some(target) = &self.target {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_progress(NavigatorGoal::new(
                    mob.get_entity().pos.load(),
                    target.position.to_centered_f64(),
                    1.0,
                ));
        }
    }
}
impl Goal for CopperTransportGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let now = world.get_world_age();
        if now < self.next_search {
            return false;
        }
        self.visited.retain(|(_, expires)| *expires > now);
        if self.visited.len() >= 10 {
            self.visited.clear();
            self.next_search = now + 140;
            return false;
        }
        let picking_up = mob.get_mob_entity().living_entity.held_item(mob).is_empty();
        let positions: Vec<BlockPos> = world
            .block_entities
            .iter()
            .flat_map(|chunk| chunk.value().keys().copied().collect::<Vec<_>>())
            .collect();
        let mut candidates: Vec<_> = positions
            .into_iter()
            .filter(|position| {
                let delta = position.to_centered_f64() - entity.pos.load();
                delta.x.abs() <= 32.0
                    && delta.z.abs() <= 32.0
                    && delta.y.abs() <= 8.0
                    && !self.visited.iter().any(|(visited, _)| visited == position)
            })
            .filter(|position| {
                let block = world.get_block(position);
                if picking_up {
                    block.has_tag(&tag::Block::MINECRAFT_COPPER_CHESTS)
                } else {
                    block.id == Block::CHEST.id || block.id == Block::TRAPPED_CHEST.id
                }
            })
            .collect();
        candidates.sort_by(|a, b| {
            entity
                .pos
                .load()
                .squared_distance_to_vec(&a.to_centered_f64())
                .total_cmp(
                    &entity
                        .pos
                        .load()
                        .squared_distance_to_vec(&b.to_centered_f64()),
                )
        });
        self.target = candidates
            .into_iter()
            .find_map(|position| ChestTarget::load(&world, position));
        self.next_search = now + if self.target.is_some() { 10 } else { 140 };
        self.target.is_some()
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.ticks_at_target = 0;
        self.path_deadline = mob.get_entity().world.load().get_world_age() + 240;
        self.navigate(mob);
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load();
        self.target.as_ref().is_some_and(|target| {
            world.get_world_age() < self.path_deadline
                && ChestTarget::load(&world, target.position).is_some()
                && self.ticks_at_target < 60
        })
    }
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(golem) = mob.cast_any().downcast_ref::<CopperGolemEntity>() else {
            return;
        };
        let Some(target) = &self.target else {
            return;
        };
        let entity = mob.get_entity();
        let world = entity.world.load();
        let center = target.position.to_centered_f64();
        let delta =
            entity.pos.load() + Vector3::new(0.0, f64::from(entity.height()) / 2.0, 0.0) - center;
        let ray = world.ray_trace_block(
            entity.pos.load() + Vector3::new(0.0, entity.get_eye_height(), 0.0),
            center,
            false,
        );
        if delta.length_squared() > 4.0
            || ray.is_some_and(|(hit, _, _)| {
                !target.parts.iter().any(|part| part.get_position() == hit)
            })
        {
            if self.ticks_at_target > 0 {
                self.ticks_at_target = 0;
                self.close(golem);
            }
            self.navigate(mob);
            return;
        }
        let busy = world
            .get_entities_at_box(&entity.bounding_box.load().expand(8.0, 8.0, 8.0))
            .iter()
            .any(|other| {
                other.get_entity().entity_id != entity.entity_id
                    && other.get_entity().is_alive()
                    && other
                        .cast_any()
                        .downcast_ref::<CopperGolemEntity>()
                        .is_some_and(|other| {
                            other.opened_chest.load().is_some_and(|position| {
                                target
                                    .parts
                                    .iter()
                                    .any(|part| part.get_position() == position)
                            })
                        })
            });
        if self.ticks_at_target == 0 && busy {
            return;
        }
        golem
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        golem
            .mob_entity
            .living_entity
            .movement_input
            .store(Vector3::new(0.0, 0.0, 0.0));
        golem
            .mob_entity
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_with_range(center.x, center.y, center.z, 45.0, 90.0);
        self.ticks_at_target += 1;
        if self.ticks_at_target == 1 {
            target.unpack_loot();
            golem.set_state(target.transfer(golem, false));
            golem.opened_chest.store(Some(target.position));
            self.opened = target.inventories();
            for inventory in &self.opened {
                inventory.on_open();
            }
        }
        if self.ticks_at_target == 9 {
            entity.play_sound(match golem.get_state() {
                CopperGolemState::GettingItem => Sound::EntityCopperGolemNoItemGet,
                CopperGolemState::GettingNoItem => Sound::EntityCopperGolemNoItemNoGet,
                CopperGolemState::DroppingItem => Sound::EntityCopperGolemItemDrop,
                _ => Sound::EntityCopperGolemItemNoDrop,
            });
        }
        if self.ticks_at_target == 60 {
            let result = target.transfer(golem, true);
            if matches!(
                result,
                CopperGolemState::GettingItem | CopperGolemState::DroppingItem
            ) {
                self.visited.clear();
            }
            self.close(golem);
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(target) = self.target.take() {
            let expires = mob.get_entity().world.load().get_world_age() + 6000;
            for part in target.parts {
                self.visited.push((part.get_position(), expires));
            }
        }
        if let Some(golem) = mob.cast_any().downcast_ref::<CopperGolemEntity>() {
            self.close(golem);
        }
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}
