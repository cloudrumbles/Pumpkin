//! Hunting, sheltering, and foraging behaviors for Minecraft's foxes.
use std::sync::{Arc, atomic::Ordering};

use pumpkin_data::{Block, entity::EntityType};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use pumpkin_world::world::BlockFlags;

use super::{Controls, Goal, is_valid_attack_target};
use crate::entity::{EntityBase, ai::pathfinder::NavigatorGoal, mob::Mob, passive::fox::FoxEntity};

fn fox(mob: &dyn Mob) -> Option<&FoxEntity> {
    mob.cast_any().downcast_ref::<FoxEntity>()
}

const LAND_PREY: &[&EntityType] = &[&EntityType::CHICKEN, &EntityType::RABBIT];
const FISH_PREY: &[&EntityType] = &[
    &EntityType::COD,
    &EntityType::SALMON,
    &EntityType::TROPICAL_FISH,
];

pub struct FoxHuntTargetGoal {
    selected: Option<Arc<dyn EntityBase>>,
    last_attempt: i64,
}

impl FoxHuntTargetGoal {
    pub const fn new() -> Self {
        Self {
            selected: None,
            last_attempt: 0,
        }
    }
}

impl Goal for FoxHuntTargetGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(fox) = fox(mob) else {
            return false;
        };
        if fox.is_sleeping() || mob.get_mob_entity().get_target().is_some() {
            return false;
        }
        let entity = mob.get_entity();
        let world = entity.world.load();
        if world.get_world_age() < self.last_attempt {
            return false;
        }
        self.last_attempt = world.get_world_age() + 20;
        // Snow foxes prefer land prey; red foxes switch between land and fish.
        let types = if fox.get_variant() == crate::entity::passive::fox::FoxVariant::Snow {
            LAND_PREY
        } else if rand::random_bool(0.3) {
            FISH_PREY
        } else {
            LAND_PREY
        };
        self.selected = world.get_nearest_entity(entity.pos.load(), 20.0, Some(types), |target| {
            is_valid_attack_target(mob, target.as_ref())
                && target.get_entity().age.load(Ordering::Relaxed) >= 0
        });
        self.selected.is_some()
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.selected.as_ref().is_some_and(|target| {
            mob.get_mob_entity().get_target().is_some_and(|current| {
                current.get_entity().entity_id == target.get_entity().entity_id
            }) && is_valid_attack_target(mob, target.as_ref())
        })
    }
    fn start(&mut self, mob: &dyn Mob) {
        if let Some(fox) = fox(mob) {
            fox.set_sleeping(false);
            fox.set_sitting(false);
        }
        mob.set_mob_target(self.selected.clone());
    }
    fn stop(&mut self, _mob: &dyn Mob) {
        self.selected = None;
    }
    fn controls(&self) -> Controls {
        Controls::TARGET
    }
}

pub struct FoxStalkGoal;

impl Goal for FoxStalkGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return false;
        };
        let Some(fox) = fox(mob) else {
            return false;
        };
        let dist = mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&target.get_entity().pos.load());
        !fox.is_pouncing()
            && !fox.is_sleeping()
            && is_valid_attack_target(mob, target.as_ref())
            && (9.0..256.0).contains(&dist)
            && mob.get_entity().on_ground.load(Ordering::Relaxed)
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return false;
        };
        let Some(fox) = fox(mob) else {
            return false;
        };
        let dist = mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&target.get_entity().pos.load());
        !fox.is_pouncing()
            && (9.0..256.0).contains(&dist)
            && is_valid_attack_target(mob, target.as_ref())
    }
    fn start(&mut self, mob: &dyn Mob) {
        if let Some(fox) = fox(mob) {
            fox.set_crouching(true);
            fox.set_interested(true);
        }
    }
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        let origin = mob.get_entity().pos.load();
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal::new(
                origin,
                target.get_entity().pos.load(),
                1.0,
            ));
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(&target, 30.0, 30.0);
    }
    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(fox) = fox(mob) {
            fox.set_crouching(false);
            fox.set_interested(false);
        }
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

pub struct FoxPounceGoal {
    target: Option<Arc<dyn EntityBase>>,
    already_hit: bool,
}

impl FoxPounceGoal {
    pub const fn new() -> Self {
        Self {
            target: None,
            already_hit: false,
        }
    }
}

impl Goal for FoxPounceGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(fox) = fox(mob) else {
            return false;
        };
        let Some(target) = mob.get_mob_entity().get_target() else {
            return false;
        };
        let delta = target.get_entity().pos.load() - mob.get_entity().pos.load();
        if !fox.is_crouching()
            || !mob.get_entity().on_ground.load(Ordering::Relaxed)
            || delta.x * delta.x + delta.z * delta.z >= 36.0
            || !is_valid_attack_target(mob, target.as_ref())
        {
            return false;
        }
        self.target = Some(target);
        true
    }
    fn start(&mut self, mob: &dyn Mob) {
        let Some(fox) = fox(mob) else {
            return;
        };
        let Some(target) = self.target.as_ref() else {
            return;
        };
        let entity = mob.get_entity();
        let delta = target.get_entity().pos.load() - entity.pos.load();
        let horizontal = delta.x.hypot(delta.z).max(0.001);
        let velocity = entity.velocity.load();
        entity.set_velocity(Vector3::new(
            delta.x / horizontal * 0.8 + velocity.x * 0.3,
            0.9,
            delta.z / horizontal * 0.8 + velocity.z * 0.3,
        ));
        fox.set_sitting(false);
        fox.set_crouching(false);
        fox.set_interested(false);
        fox.set_pouncing(true);
        self.already_hit = false;
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.target
            .as_ref()
            .is_some_and(|target| is_valid_attack_target(mob, target.as_ref()))
            && !mob.get_entity().on_ground.load(Ordering::Relaxed)
    }
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = self.target.as_ref() else {
            return;
        };
        let entity = mob.get_entity();
        if !self.already_hit
            && entity
                .bounding_box
                .load()
                .expand(0.35, 0.35, 0.35)
                .intersects(&target.get_entity().bounding_box.load())
        {
            mob.get_mob_entity().try_attack(mob, target.as_ref());
            self.already_hit = true;
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(fox) = fox(mob) {
            fox.set_pouncing(false);
            fox.set_interested(false);
        }
        self.target = None;
        self.already_hit = false;
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::JUMP | Controls::LOOK
    }
}

pub struct FoxSleepGoal;

impl FoxSleepGoal {
    fn can_sleep(mob: &dyn Mob) -> bool {
        let Some(fox) = fox(mob) else {
            return false;
        };
        let entity = mob.get_entity();
        let world = entity.world.load();
        let day = world.get_time_of_day().rem_euclid(24000) < 12000;
        day && mob.get_mob_entity().get_target().is_none()
            && entity.on_ground.load(Ordering::Relaxed)
            && !world.can_see_sky(&entity.block_pos.load())
            && world
                .get_nearest_player(entity.pos.load(), 8.0, |p| !p.is_spectator())
                .is_none()
            && !fox.is_pouncing()
    }
}

impl Goal for FoxSleepGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        Self::can_sleep(mob) && rand::random_range(0..100) == 0
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        Self::can_sleep(mob)
    }
    fn start(&mut self, mob: &dyn Mob) {
        if let Some(fox) = fox(mob) {
            fox.clear_states();
            fox.set_sleeping(true);
        }
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
    }
    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(fox) = fox(mob) {
            fox.set_sleeping(false);
        }
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK | Controls::JUMP
    }
}

pub struct FoxEatBerriesGoal {
    target: Option<BlockPos>,
    ticks_at_bush: i32,
    next_attempt: i64,
}

impl FoxEatBerriesGoal {
    pub const fn new() -> Self {
        Self {
            target: None,
            ticks_at_bush: 0,
            next_attempt: 0,
        }
    }
    fn berry_age(world: &crate::world::World, pos: BlockPos) -> Option<i32> {
        let (block, state) = world.get_block_and_state_id(&pos);
        if block != &Block::SWEET_BERRY_BUSH {
            return None;
        }
        block
            .properties(state)?
            .to_props()
            .iter()
            .find(|(name, _)| *name == "age")
            .and_then(|(_, age)| age.parse().ok())
    }
}

impl Goal for FoxEatBerriesGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(fox) = fox(mob) else {
            return false;
        };
        let entity = mob.get_entity();
        let world = entity.world.load();
        if fox.is_sleeping()
            || mob.get_mob_entity().get_target().is_some()
            || !world.level_info.load().game_rules.mob_griefing
            || world.get_world_age() < self.next_attempt
        {
            return false;
        }
        self.next_attempt = world.get_world_age() + 80;
        let current = entity.block_pos.load();
        let mut closest: Option<(i32, BlockPos)> = None;
        for dx in -10..=10 {
            for dz in -10..=10 {
                for dy in -2..=2 {
                    let pos = BlockPos::new(current.0.x + dx, current.0.y + dy, current.0.z + dz);
                    if Self::berry_age(&world, pos).is_some_and(|age| age >= 2) {
                        let dist = dx * dx + dz * dz + dy * dy;
                        if closest.as_ref().is_none_or(|(best, _)| dist < *best) {
                            closest = Some((dist, pos));
                        }
                    }
                }
            }
        }
        self.target = closest.map(|(_, pos)| pos);
        self.target.is_some()
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(target) = self.target else {
            return false;
        };
        !fox(mob).is_some_and(FoxEntity::is_sleeping)
            && Self::berry_age(&mob.get_entity().world.load(), target).is_some_and(|age| age >= 2)
    }
    fn start(&mut self, _mob: &dyn Mob) {
        self.ticks_at_bush = 0;
    }
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = self.target else {
            return;
        };
        let entity = mob.get_entity();
        let here = entity.pos.load();
        let dest = target.to_centered_f64();
        if here.squared_distance_to_vec(&dest) > 4.0 {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_progress(NavigatorGoal::new(here, dest, 1.2));
            return;
        }
        self.ticks_at_bush += 1;
        if self.ticks_at_bush < 40 {
            return;
        }
        let world = entity.world.load_full();
        if Self::berry_age(&world, target).is_some_and(|age| age >= 2) {
            let fields = [("age", "1")];
            world.set_block_state(
                &target,
                Block::SWEET_BERRY_BUSH
                    .from_properties(&fields)
                    .to_state_id(&Block::SWEET_BERRY_BUSH),
                BlockFlags::NOTIFY_ALL,
            );
            world.emit_game_event_with_source("minecraft:block_change", dest, Some(entity));
        }
        self.target = None;
        self.next_attempt = world.get_world_age() + 200;
    }
    fn stop(&mut self, mob: &dyn Mob) {
        self.target = None;
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
    }
    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// A fox remembers up to two people who bred it and retaliates against mobs
/// that recently hurt either of them.
pub struct FoxDefendTrustedGoal {
    last_time: i32,
    target: Option<Arc<dyn EntityBase>>,
}
impl FoxDefendTrustedGoal {
    pub const fn new() -> Self {
        Self {
            last_time: 0,
            target: None,
        }
    }
}
impl Goal for FoxDefendTrustedGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(fox) = fox(mob) else {
            return false;
        };
        let entity = mob.get_entity();
        let world = entity.world.load();
        let trusted = fox
            .trusted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for id in trusted.iter() {
            let Some(player) = world.get_player_by_uuid(*id) else {
                continue;
            };
            if player
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&entity.pos.load())
                > 16.0 * 16.0
            {
                continue;
            }
            let when = player
                .living_entity
                .last_attacked_time
                .load(Ordering::Relaxed);
            if when == self.last_time {
                continue;
            }
            let attacker_id = player
                .living_entity
                .last_attacker_id
                .load(Ordering::Relaxed);
            let Some(attacker) = world.get_entity_by_id(attacker_id) else {
                continue;
            };
            if !is_valid_attack_target(mob, attacker.as_ref())
                || fox.trusts(attacker.get_entity().entity_uuid)
            {
                continue;
            }
            self.last_time = when;
            self.target = Some(attacker);
            return true;
        }
        false
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.target
            .as_ref()
            .is_some_and(|target| is_valid_attack_target(mob, target.as_ref()))
    }
    fn start(&mut self, mob: &dyn Mob) {
        if let Some(fox) = fox(mob) {
            fox.clear_states();
            fox.set_defending(true);
        }
        mob.set_mob_target(self.target.clone());
    }
    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(fox) = fox(mob) {
            fox.set_defending(false);
        }
        self.target = None;
    }
    fn controls(&self) -> Controls {
        Controls::TARGET
    }
}
