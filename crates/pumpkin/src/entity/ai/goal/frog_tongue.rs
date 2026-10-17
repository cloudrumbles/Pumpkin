use super::{Controls, Goal, is_valid_attack_target};
use crate::entity::{
    EntityBase, ai::pathfinder::NavigatorGoal, mob::Mob, passive::frog::FrogEntity,
};
use pumpkin_data::{
    entity::{EntityPose, EntityType},
    sound::Sound,
    tag::{self, Taggable},
};
use pumpkin_util::math::vector3::Vector3;
use std::{
    collections::VecDeque,
    sync::{Arc, atomic::Ordering},
};
use uuid::Uuid;

#[derive(Default)]
pub struct FrogTongueGoal {
    target: Option<Arc<dyn EntityBase>>,
    elapsed: i32,
    animation: Option<i32>,
    unreachable: VecDeque<(Uuid, i64)>,
}

impl FrogTongueGoal {
    fn can_eat(mob: &dyn Mob, target: &dyn EntityBase) -> bool {
        let entity = target.get_entity();
        let cube = [
            &EntityType::SLIME,
            &EntityType::MAGMA_CUBE,
            &EntityType::SULFUR_CUBE,
        ]
        .contains(&entity.entity_type);
        is_valid_attack_target(mob, target)
            && entity
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_FROG_FOOD)
            && (!cube || entity.data.load(Ordering::Relaxed) <= 1)
    }
}

impl Goal for FrogTongueGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let now = world.get_world_age();
        self.unreachable.retain(|(_, until)| *until > now);
        if mob
            .get_mob_entity()
            .living_entity
            .hurt_cooldown
            .load(Ordering::Relaxed)
            > 0
            || mob.get_mob_entity().is_in_love()
        {
            return false;
        }
        self.target = world.get_nearest_entity(entity.pos.load(), 16.0, None, |target| {
            Self::can_eat(mob, target.as_ref())
                && !self
                    .unreachable
                    .iter()
                    .any(|(id, _)| *id == target.get_entity().entity_uuid)
                && mob.has_line_of_sight(target.get_entity())
        });
        if let Some(target) = &self.target {
            let reachable = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .can_reach_within(
                    &mob.get_mob_entity().living_entity,
                    target.get_entity().pos.load(),
                    1.75,
                );
            if !reachable {
                if self.unreachable.len() >= 5 {
                    self.unreachable.pop_front();
                }
                self.unreachable
                    .push_back((target.get_entity().entity_uuid, now + 100));
                self.target = None;
            }
        }
        self.target.is_some()
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.elapsed = 0;
        self.animation = None;
        if let Some(target) = &self.target {
            mob.get_mob_entity().set_target(Some(target.clone()));
            if let Some(frog) = mob.cast_any().downcast_ref::<FrogEntity>() {
                frog.set_tongue_target(Some(target.get_entity().entity_id));
            }
        }
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.elapsed < 100
            && self.animation.is_none_or(|tick| tick <= 10)
            && self
                .target
                .as_ref()
                .is_some_and(|t| self.animation.is_some() || Self::can_eat(mob, t.as_ref()))
    }
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = &self.target else {
            return;
        };
        self.elapsed += 1;
        let entity = mob.get_entity();
        let pos = entity.pos.load();
        let target_pos = target.get_entity().pos.load();
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(target, 45.0, 90.0);
        if let Some(timer) = self.animation.as_mut() {
            *timer += 1;
            if *timer == 6
                && Self::can_eat(mob, target.as_ref())
                && pos.squared_distance_to_vec(&target_pos) < 4.0
                && mob.has_line_of_sight(target.get_entity())
            {
                entity.play_sound(Sound::EntityFrogEat);
                mob.get_mob_entity().try_attack(mob, target.as_ref());
                if target
                    .get_living_entity()
                    .is_some_and(|living| !living.is_alive())
                {
                    target.get_entity().remove();
                }
            }
        } else if pos.squared_distance_to_vec(&target_pos) < 1.75 * 1.75
            && mob.has_line_of_sight(target.get_entity())
        {
            entity.play_sound(Sound::EntityFrogTongue);
            entity.set_pose(EntityPose::UsingTongue);
            target
                .get_entity()
                .set_velocity((pos - target_pos).normalize() * 0.75);
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
            mob.get_mob_entity()
                .living_entity
                .movement_input
                .store(Vector3::new(0.0, 0.0, 0.0));
            self.animation = Some(0);
        } else if self.elapsed % 10 == 1 {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_progress(NavigatorGoal::new(pos, target_pos, 2.0));
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        if self.elapsed >= 100
            && let Some(target) = &self.target
        {
            if self.unreachable.len() >= 5 {
                self.unreachable.pop_front();
            }
            self.unreachable.push_back((
                target.get_entity().entity_uuid,
                mob.get_entity().world.load().get_world_age() + 100,
            ));
        }
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        mob.get_mob_entity().set_target(None);
        if let Some(frog) = mob.cast_any().downcast_ref::<FrogEntity>() {
            frog.set_tongue_target(None);
        }
        mob.get_entity().set_pose(EntityPose::Standing);
        self.target = None;
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}
