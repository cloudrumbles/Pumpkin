use super::{Controls, Goal};

use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::mob::Mob;
use crate::entity::predicate::EntityPredicate;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

const MAX_ATTACK_TIME: i64 = 20;

pub struct MeleeAttackGoal {
    goal_control: Controls,
    speed: f64,
    pause_when_mob_idle: bool,
    path: Option<Path>,
    update_countdown_ticks: i32,
    pub cooldown: i32,
    attack_handler: Option<fn(&dyn Mob, &dyn EntityBase, &mut Self)>,
    last_update_time: i64,
    last_target_position: Option<Vector3<f64>>,
}

impl MeleeAttackGoal {
    #[must_use]
    pub fn new(speed: f64, pause_when_mob_idle: bool) -> Self {
        Self {
            goal_control: Controls::MOVE | Controls::LOOK,
            speed,
            pause_when_mob_idle,
            path: None,
            update_countdown_ticks: 0,
            cooldown: 0,
            attack_handler: None,
            last_update_time: 0,
            last_target_position: None,
        }
    }

    #[must_use]
    pub fn get_max_cooldown(&self) -> i32 {
        self.get_tick_count(20)
    }

    #[must_use]
    pub fn with_attack_handler(
        mut self,
        handler: fn(&dyn Mob, &dyn EntityBase, &mut Self),
    ) -> Self {
        self.attack_handler = Some(handler);
        self
    }

    pub fn can_perform_attack(&self, mob: &dyn Mob, target: &dyn EntityBase) -> bool {
        self.cooldown <= 0
            && super::is_valid_attack_target(mob, target)
            && mob.get_mob_entity().is_in_attack_range(target)
            && mob.has_line_of_sight(target.get_entity())
    }

    pub fn perform_attack(&mut self, mob: &dyn Mob, target: &dyn EntityBase) {
        self.cooldown = self.get_max_cooldown();
        mob.get_mob_entity().living_entity.swing_hand();
        mob.get_mob_entity().try_attack(mob, target);
    }
}

impl Goal for MeleeAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let time = mob.get_entity().world.load().get_world_age();

        if time - self.last_update_time < MAX_ATTACK_TIME {
            return false;
        }
        self.last_update_time = time;

        let target = mob.get_mob_entity().get_target();

        let Some(target) = target.as_ref() else {
            return false;
        };
        if !super::is_valid_attack_target(mob, target.as_ref()) {
            return false;
        }
        self.path = mob
            .get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .create_path(
                &mob.get_mob_entity().living_entity,
                target.get_entity().pos.load(),
                0,
            );
        self.path.is_some() || mob.get_mob_entity().is_in_attack_range(target.as_ref())
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let target = mob.get_mob_entity().get_target().clone();

        let Some(target) = target else {
            return false;
        };
        if !super::is_valid_attack_target(mob, target.as_ref()) {
            return false;
        }

        if !self.pause_when_mob_idle {
            let is_idle = mob.is_navigator_idle();
            return !is_idle;
        }

        let is_valid_target = !target
            .get_player()
            .is_some_and(|p| p.is_spectator() || p.is_creative());

        let in_range = mob
            .get_mob_entity()
            .is_in_position_target_range_pos(&target.get_entity().block_pos.load());

        in_range && is_valid_target
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity().set_attacking(true);

        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .move_to_path(
                self.path.take(),
                self.speed,
                &mob.get_mob_entity().living_entity,
            );
        self.last_target_position = None;
        self.update_countdown_ticks = 0;
        self.cooldown = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        // Only clear target if they switched to creative/spectator
        let should_clear = mob
            .get_mob_entity()
            .get_target()
            .as_deref()
            .is_some_and(|entity| {
                !EntityPredicate::ExceptCreativeOrSpectator.test(entity.get_entity())
            });
        if should_clear {
            mob.set_mob_target(None);
        }

        mob.get_mob_entity().set_attacking(false);

        // Vanilla: this.mob.getNavigation().stop()
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        self.last_target_position = None;
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let target = mob.get_mob_entity().get_target().clone();
        let Some(target) = target else {
            return;
        };

        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(&target, 30.0, 30.0);

        self.update_countdown_ticks = (self.update_countdown_ticks - 1).max(0);

        let current_target_pos = target.get_entity().pos.load();
        let has_line_of_sight =
            self.pause_when_mob_idle || mob.has_line_of_sight(target.get_entity());
        let should_update_nav = has_line_of_sight
            && self.update_countdown_ticks <= 0
            && (self.last_target_position.is_none_or(|last_pos| {
                current_target_pos.squared_distance_to_vec(&last_pos) >= 1.0
            }) || mob.get_random().random::<f32>() < 0.05);

        if should_update_nav {
            let mob_pos = mob.get_entity().pos.load();
            let dist_sq = mob_pos.squared_distance_to_vec(&current_target_pos);
            let mut navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let moved = navigator.move_to_coords(
                current_target_pos.x,
                current_target_pos.y,
                current_target_pos.z,
                self.speed,
                &mob.get_mob_entity().living_entity,
            );
            self.last_target_position = Some(current_target_pos);
            self.update_countdown_ticks = 4 + mob.get_random().random_range(0..7);
            if dist_sq > 1024.0 {
                self.update_countdown_ticks += 10;
            } else if dist_sq > 256.0 {
                self.update_countdown_ticks += 5;
            }
            if !moved {
                self.update_countdown_ticks += 15;
            }
            self.update_countdown_ticks = self.get_tick_count(self.update_countdown_ticks);
        }

        self.cooldown = (self.cooldown - 1).max(0);

        if let Some(handler) = self.attack_handler {
            handler(mob, target.as_ref(), self);
        } else if self.can_perform_attack(mob, target.as_ref()) {
            self.perform_attack(mob, target.as_ref());
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}
