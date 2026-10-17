use super::{
    Controls, Goal, is_valid_attack_target,
    long_jump::{cooldown_ready, set_cooldown},
};
use crate::entity::{
    EntityBase,
    ai::{brain::memory::types, pathfinder::NavigatorGoal},
    mob::Mob,
    passive::goat::GoatEntity,
};
use pumpkin_data::{
    attributes::Attributes,
    damage::DamageType,
    effect::StatusEffect,
    entity::{EntityStatus, EntityType},
    sound::Sound,
    tag::{self, Taggable},
};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use std::sync::{Arc, atomic::Ordering};

#[derive(Default)]
pub struct GoatRamGoal {
    target: Option<Arc<dyn EntityBase>>,
    start: Option<Vector3<f64>>,
    target_block: Option<BlockPos>,
    direction: Option<Vector3<f64>>,
    elapsed: i32,
    prepare: i32,
    ramming: bool,
    finished: bool,
}

impl GoatRamGoal {
    fn valid(mob: &dyn Mob, target: &dyn EntityBase) -> bool {
        let entity = target.get_entity();
        entity.entity_type != &EntityType::GOAT
            && is_valid_attack_target(mob, target)
            && (entity.entity_type != &EntityType::ARMOR_STAND
                || entity
                    .world
                    .load()
                    .level_info
                    .load()
                    .game_rules
                    .mob_griefing)
    }
    fn screaming(mob: &dyn Mob) -> bool {
        mob.cast_any()
            .downcast_ref::<GoatEntity>()
            .is_some_and(GoatEntity::is_screaming)
    }
    fn find_start(mob: &dyn Mob, target: BlockPos) -> Option<Vector3<f64>> {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let walkable = |block: BlockPos| {
            world.get_block(&block.down()).is_solid()
                && world.get_block(&block).is_air()
                && world.get_block(&block.up()).is_air()
                && world.is_space_empty(pumpkin_util::math::boundingbox::BoundingBox::new(
                    entity.bounding_box.load().min
                        + Vector3::new(
                            f64::from(block.0.x) + 0.5,
                            f64::from(block.0.y),
                            f64::from(block.0.z) + 0.5,
                        )
                        - entity.pos.load(),
                    entity.bounding_box.load().max
                        + Vector3::new(
                            f64::from(block.0.x) + 0.5,
                            f64::from(block.0.y),
                            f64::from(block.0.z) + 0.5,
                        )
                        - entity.pos.load(),
                ))
        };
        if !walkable(target) {
            return None;
        }
        let mut starts = Vec::new();
        for direction in [
            Vector3::new(1, 0, 0),
            Vector3::new(-1, 0, 0),
            Vector3::new(0, 0, 1),
            Vector3::new(0, 0, -1),
        ] {
            let mut distance = 0;
            for step in 1..=7 {
                if walkable(target.offset(direction * step)) {
                    distance = step;
                } else {
                    break;
                }
            }
            if distance >= 4 {
                let block = target.offset(direction * distance);
                starts.push(Vector3::new(
                    f64::from(block.0.x) + 0.5,
                    f64::from(block.0.y),
                    f64::from(block.0.z) + 0.5,
                ));
            }
        }
        starts.sort_by(|a, b| {
            entity
                .pos
                .load()
                .squared_distance_to_vec(a)
                .total_cmp(&entity.pos.load().squared_distance_to_vec(b))
        });
        starts.into_iter().find(|pos| {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .can_reach_within(&mob.get_mob_entity().living_entity, *pos, 0.0)
        })
    }
    fn finish(&mut self, mob: &dyn Mob, success: bool) {
        let entity = mob.get_entity();
        entity
            .world
            .load()
            .send_entity_status(entity, EntityStatus::EndRam, None);
        let ticks = if Self::screaming(mob) {
            if success {
                rand::random_range(100..=300)
            } else {
                100
            }
        } else if success {
            rand::random_range(600..=6000)
        } else {
            600
        };
        set_cooldown(mob, types::RAM_COOLDOWN_TICKS, ticks);
        mob.get_mob_entity()
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .erase(types::RAM_TARGET.id());
        self.finished = true;
    }
}

impl Goal for GoatRamGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if !cooldown_ready(mob, types::RAM_COOLDOWN_TICKS)
            || mob.get_mob_entity().is_in_love()
            || mob.get_entity().touching_water.load(Ordering::Relaxed)
        {
            return false;
        }
        let entity = mob.get_entity();
        let world = entity.world.load();
        self.target = world.get_nearest_entity(entity.pos.load(), 16.0, None, |t| {
            Self::valid(mob, t.as_ref()) && mob.has_line_of_sight(t.get_entity())
        });
        if let Some(target) = &self.target {
            let block = target.get_entity().block_pos.load();
            self.target_block = Some(block);
            self.start = Self::find_start(mob, block);
        } else {
            self.start = None;
        }
        if self.start.is_none() {
            self.finish(mob, false);
        }
        self.start.is_some()
    }
    fn start(&mut self, _mob: &dyn Mob) {
        self.elapsed = 0;
        self.prepare = 0;
        self.ramming = false;
        self.finished = false;
        self.direction = None;
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !self.finished
            && self.elapsed < 200
            && (self.ramming
                || self
                    .target
                    .as_ref()
                    .is_some_and(|target| Self::valid(mob, target.as_ref())))
    }
    fn tick(&mut self, mob: &dyn Mob) {
        self.elapsed += 1;
        let Some(target) = self.target.clone() else {
            return;
        };
        let entity = mob.get_entity();
        let pos = entity.pos.load();
        let world = entity.world.load();
        if !self.ramming {
            if Some(target.get_entity().block_pos.load()) != self.target_block {
                self.finish(mob, false);
                return;
            }
            let Some(start) = self.start else {
                self.finish(mob, false);
                return;
            };
            mob.get_mob_entity()
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at_entity_with_range(&target, 45.0, 90.0);
            if pos.squared_distance_to_vec(&start) < 0.64 {
                mob.get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stop();
                mob.get_mob_entity()
                    .living_entity
                    .movement_input
                    .store(Vector3::new(0.0, 0.0, 0.0));
                if self.prepare == 0 {
                    world.send_entity_status(entity, EntityStatus::StartRam, None);
                }
                self.prepare += 1;
                if self.prepare >= 20 {
                    let target_pos = target.get_entity().pos.load();
                    let direction =
                        Vector3::new(target_pos.x - pos.x, 0.0, target_pos.z - pos.z).normalize();
                    let destination = target_pos + direction * 0.5;
                    self.direction = Some(direction);
                    self.ramming = true;
                    self.elapsed = 0;
                    mob.get_mob_entity()
                        .brain
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .set(types::RAM_TARGET, destination);
                    mob.get_mob_entity()
                        .navigator
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .set_progress(NavigatorGoal::new(pos, destination, 3.0));
                    entity.play_sound(if Self::screaming(mob) {
                        Sound::EntityGoatScreamingPrepareRam
                    } else {
                        Sound::EntityGoatPrepareRam
                    });
                }
            } else if self.elapsed % 10 == 1 {
                mob.get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set_progress(NavigatorGoal::new(pos, start, 1.25));
            }
            if self.elapsed >= 160 {
                self.finish(mob, false);
            }
            return;
        }
        if let Some(victim) = world
            .get_all_at_box(&entity.bounding_box.load().expand(0.1, 0.0, 0.1))
            .into_iter()
            .find(|candidate| Self::valid(mob, candidate.as_ref()))
        {
            let living = &mob.get_mob_entity().living_entity;
            let damage = living.get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
            victim.damage_with_context(
                victim.as_ref(),
                damage,
                DamageType::MOB_ATTACK_NO_AGGRO,
                Some(pos),
                Some(mob),
                Some(mob),
            );
            let speed_level = living
                .get_effect(&StatusEffect::SPEED)
                .map_or(0, |e| i32::from(e.amplifier) + 1);
            let slow_level = living
                .get_effect(&StatusEffect::SLOWNESS)
                .map_or(0, |e| i32::from(e.amplifier) + 1);
            let speed = (living.get_attribute_value(&Attributes::MOVEMENT_SPEED) * 3.0 * 1.65)
                .clamp(0.2, 3.0)
                + 0.25 * f64::from(speed_level - slow_level);
            let force = if mob
                .as_ageable()
                .is_some_and(crate::entity::ageable::AgeableMob::is_baby)
            {
                1.0
            } else {
                2.5
            };
            if let Some(direction) = self.direction {
                victim
                    .get_entity()
                    .knockback(speed * force, -direction.x, -direction.z);
            }
            entity.play_sound(if Self::screaming(mob) {
                Sound::EntityGoatScreamingRamImpact
            } else {
                Sound::EntityGoatRamImpact
            });
            self.finish(mob, true);
        } else if let Some(direction) = self.direction {
            let front = BlockPos::floored_v(pos + direction);
            if world
                .get_block(&front)
                .has_tag(&tag::Block::MINECRAFT_SNAPS_GOAT_HORN)
                || world
                    .get_block(&front.up())
                    .has_tag(&tag::Block::MINECRAFT_SNAPS_GOAT_HORN)
            {
                if let Some(goat) = mob.cast_any().downcast_ref::<GoatEntity>() {
                    goat.drop_horn();
                }
                entity.play_sound(if Self::screaming(mob) {
                    Sound::EntityGoatScreamingRamImpact
                } else {
                    Sound::EntityGoatRamImpact
                });
                self.finish(mob, true);
            } else if self.elapsed > 2
                && (mob.is_navigator_idle() || entity.horizontal_collision.load(Ordering::Relaxed))
            {
                self.finish(mob, true);
            }
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        if !self.finished {
            self.finish(mob, false);
        }
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        self.target = None;
        self.start = None;
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}
