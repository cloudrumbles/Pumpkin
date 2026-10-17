use std::sync::atomic::Ordering;

use pumpkin_data::{
    attributes::Attributes,
    entity::{EntityPose, EntityType},
    sound::Sound,
    tag::{self, Taggable},
};
use pumpkin_util::math::{boundingbox::BoundingBox, position::BlockPos, vector3::Vector3};

use super::{Controls, Goal};
use crate::entity::{
    ai::brain::{
        Brain,
        memory::{MemoryModuleType, types},
    },
    mob::Mob,
    passive::goat::GoatEntity,
};

/// These counters are ordinary, serializable vanilla brain memories. They stop
/// when NoAI is set, and resume after an unload or restart.
pub fn tick_cooldown(mob: &dyn Mob, key: MemoryModuleType<i32>) {
    let mut brain = mob
        .get_mob_entity()
        .brain
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(value) = brain.get(key).copied() {
        if value <= 1 {
            brain.erase(key.id());
        } else {
            brain.set(key, value - 1);
        }
    }
}

pub fn cooldown_ready(mob: &dyn Mob, key: MemoryModuleType<i32>) -> bool {
    mob.get_mob_entity()
        .brain
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(key)
        .is_none_or(|value| *value <= 0)
}

pub fn set_cooldown(mob: &dyn Mob, key: MemoryModuleType<i32>, value: i32) {
    let mut brain = mob
        .get_mob_entity()
        .brain
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    brain.register_memory(key.id());
    brain.set(key, value);
}

pub fn register_jump_memories(brain: &mut Brain, minimum: i32, maximum: i32) {
    brain.register_memory(types::LONG_JUMP_COOLDOWN_TICKS.id());
    brain.register_memory(types::LONG_JUMP_MID_JUMP.id());
    brain.set(
        types::LONG_JUMP_COOLDOWN_TICKS,
        rand::random_range(minimum..=maximum),
    );
}

/// Integrate the same per-tick, gravity-only motion as LivingEntity. Sampling
/// between tick positions prevents a fast jump tunnelling through a thin wall.
pub fn clear_jump(
    mob: &dyn Mob,
    landing: Vector3<f64>,
    ticks: i32,
    max_speed: f64,
) -> Option<Vector3<f64>> {
    let entity = mob.get_entity();
    let pos = entity.pos.load();
    let gravity = mob.get_mob_gravity();
    if ticks < 2 || gravity <= 0.0 {
        return None;
    }
    let delta = landing - pos;
    let time = f64::from(ticks);
    let velocity = Vector3::new(
        delta.x / time,
        delta.y / time + gravity * (time - 1.0) / 2.0,
        delta.z / time,
    );
    if velocity.length_squared() > max_speed * max_speed || velocity.y <= 0.0 {
        return None;
    }
    let world = entity.world.load();
    let bounds = entity.bounding_box.load();
    let mut point = pos;
    let mut step = velocity;
    for tick in 0..ticks {
        let slices = (step.length() / 0.2).ceil().max(1.0) as i32;
        for slice in 1..=slices {
            let at = point + step * (f64::from(slice) / f64::from(slices));
            // At the last sample the feet may touch the landing surface.
            let tolerance = if tick == ticks - 1 { 0.01 } else { 0.0 };
            let shifted = BoundingBox::new(
                bounds.min + at - pos + Vector3::new(0.0, tolerance, 0.0),
                bounds.max + at - pos,
            );
            if !world.is_space_empty(shifted) {
                return None;
            }
        }
        point += step;
        step.y -= gravity;
    }
    Some(velocity)
}

pub struct LongJumpGoal {
    width: i32,
    height: i32,
    minimum: i32,
    maximum: i32,
    velocity: Option<Vector3<f64>>,
    origin: Vector3<f64>,
    timer: i32,
    launched: bool,
}

impl LongJumpGoal {
    pub fn new(width: i32, height: i32, minimum: i32, maximum: i32) -> Box<Self> {
        Box::new(Self {
            width,
            height,
            minimum,
            maximum,
            velocity: None,
            origin: Vector3::new(0.0, 0.0, 0.0),
            timer: 0,
            launched: false,
        })
    }

    fn reset_cooldown(&self, mob: &dyn Mob, failed: bool) {
        let ticks = rand::random_range(self.minimum..=self.maximum);
        set_cooldown(
            mob,
            types::LONG_JUMP_COOLDOWN_TICKS,
            if failed { ticks / 2 } else { ticks },
        );
    }
}

impl Goal for LongJumpGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        if !cooldown_ready(mob, types::LONG_JUMP_COOLDOWN_TICKS)
            || mob.get_mob_entity().is_in_love()
            || mob.get_mob_entity().get_target().is_some()
            || entity.has_passengers()
        {
            return false;
        }
        if !entity.on_ground.load(Ordering::Relaxed)
            || entity.touching_water.load(Ordering::Relaxed)
        {
            self.reset_cooldown(mob, true);
            return false;
        }
        let world = entity.world.load();
        let pos = entity.pos.load();
        let is_frog = entity.entity_type == &EntityType::FROG;
        let max_speed = mob
            .get_mob_entity()
            .living_entity
            .get_attribute_value(&Attributes::JUMP_STRENGTH)
            * 3.571_428_8;
        let mut candidates = Vec::new();
        for _ in 0..32 {
            let x = pos.x.floor() as i32 + rand::random_range(-self.width..=self.width);
            let z = pos.z.floor() as i32 + rand::random_range(-self.width..=self.width);
            if x == pos.x.floor() as i32 && z == pos.z.floor() as i32 {
                continue;
            }
            for y in (pos.y.floor() as i32 - self.height..=pos.y.floor() as i32 + self.height).rev()
            {
                let block = BlockPos::new(x, y, z);
                let below = world.get_block(&block.down());
                if !below.is_solid() || !world.get_block(&block).is_air() {
                    continue;
                }
                let landing = Vector3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
                let preferred =
                    is_frog && below.has_tag(&tag::Block::MINECRAFT_FROG_PREFER_JUMP_TO);
                candidates.push((preferred, landing));
                break;
            }
        }
        if is_frog && rand::random::<bool>() {
            candidates.sort_by_key(|(preferred, _)| !preferred);
        }
        for (_, landing) in candidates {
            // Goats reserve long jumps for destinations not reachable by walking.
            if !is_frog
                && mob
                    .get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .can_reach_within(&mob.get_mob_entity().living_entity, landing, 0.0)
            {
                continue;
            }
            for ticks in [20, 24, 16, 28] {
                if let Some(velocity) = clear_jump(mob, landing, ticks, max_speed) {
                    self.velocity = Some(velocity);
                    self.origin = pos;
                    return true;
                }
            }
        }
        self.reset_cooldown(mob, true);
        false
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.timer = 0;
        self.launched = false;
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        mob.get_mob_entity()
            .living_entity
            .movement_input
            .store(Vector3::new(0.0, 0.0, 0.0));
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        if self.launched {
            self.timer < 100
                && (self.timer < 43 || !mob.get_entity().on_ground.load(Ordering::Relaxed))
        } else {
            self.timer <= 40
                && mob
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&self.origin)
                    < 0.04
                && !mob.get_entity().touching_water.load(Ordering::Relaxed)
        }
    }
    fn tick(&mut self, mob: &dyn Mob) {
        self.timer += 1;
        if self.timer == 40
            && let Some(mut velocity) = self.velocity
        {
            let living = &mob.get_mob_entity().living_entity;
            if let Some(boost) = living.get_effect(&pumpkin_data::effect::StatusEffect::JUMP_BOOST)
            {
                let length = velocity.length();
                if length > 0.0 {
                    velocity =
                        velocity * ((length + 0.1 * f64::from(boost.amplifier + 1)) / length);
                }
            }
            living.discard_friction.store(true, Ordering::Relaxed);
            living.entity.on_ground.store(false, Ordering::Relaxed);
            living.entity.set_pose(EntityPose::LongJumping);
            living.entity.set_velocity(velocity);
            let sound = if living.entity.entity_type == &EntityType::FROG {
                Sound::EntityFrogLongJump
            } else if mob
                .cast_any()
                .downcast_ref::<GoatEntity>()
                .is_some_and(GoatEntity::is_screaming)
            {
                Sound::EntityGoatScreamingLongJump
            } else {
                Sound::EntityGoatLongJump
            };
            living.entity.play_sound(sound);
            mob.get_mob_entity()
                .brain
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set(types::LONG_JUMP_MID_JUMP, true);
            self.launched = true;
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        let living = &mob.get_mob_entity().living_entity;
        living.discard_friction.store(false, Ordering::Relaxed);
        living.entity.set_pose(EntityPose::Standing);
        if self.launched && living.entity.on_ground.load(Ordering::Relaxed) {
            let mut velocity = living.entity.velocity.load();
            velocity.x *= 0.1;
            velocity.z *= 0.1;
            living.entity.set_velocity(velocity);
            living
                .entity
                .play_sound(if living.entity.entity_type == &EntityType::FROG {
                    Sound::EntityFrogStep
                } else {
                    Sound::EntityGoatStep
                });
        }
        mob.get_mob_entity()
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .erase(types::LONG_JUMP_MID_JUMP.id());
        self.reset_cooldown(mob, !self.launched);
        self.velocity = None;
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn can_stop(&self) -> bool {
        !self.launched
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK | Controls::JUMP
    }
}
