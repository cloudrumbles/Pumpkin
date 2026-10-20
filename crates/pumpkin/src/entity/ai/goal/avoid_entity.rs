use std::sync::Arc;

use super::{Controls, Goal};
use crate::entity::ai::util::default_random_pos;
use crate::entity::predicate::EntityPredicate;
use crate::entity::{EntityBase, ai::pathfinder::NavigatorGoal, mob::Mob};
use pumpkin_data::entity::EntityType;
use pumpkin_util::math::vector3::Vector3;

const FAST_DISTANCE_SQ: f64 = 49.0;
const HORIZONTAL_RANGE: i32 = 16;
const VERTICAL_RANGE: i32 = 7;

pub struct AvoidEntityGoal {
    goal_control: Controls,
    flee_type: Option<&'static EntityType>,
    flee_filter: fn(&dyn Mob, &dyn EntityBase) -> bool,
    flee_distance: f64,
    slow_speed: f64,
    fast_speed: f64,
    target: Option<Arc<dyn EntityBase>>,
    flee_pos: Option<Vector3<f64>>,
}

impl AvoidEntityGoal {
    #[must_use]
    pub fn new(
        flee_type: &'static EntityType,
        flee_distance: f64,
        slow_speed: f64,
        fast_speed: f64,
    ) -> Self {
        Self {
            goal_control: Controls::MOVE,
            flee_type: Some(flee_type),
            flee_filter: |_, _| true,
            flee_distance,
            slow_speed,
            fast_speed,
            target: None,
            flee_pos: None,
        }
    }

    #[must_use]
    pub fn with_filter(
        flee_distance: f64,
        slow_speed: f64,
        fast_speed: f64,
        filter: fn(&dyn Mob, &dyn EntityBase) -> bool,
    ) -> Self {
        let mut goal = Self::new(&EntityType::PLAYER, flee_distance, slow_speed, fast_speed);
        goal.flee_type = None;
        goal.flee_filter = filter;
        goal
    }

    fn find_threat(&self, mob: &dyn Mob) -> Option<Arc<dyn EntityBase>> {
        let entity = &mob.get_mob_entity().living_entity.entity;
        let pos = entity.pos.load();
        let world = entity.world.load();

        if self.flee_type == Some(&EntityType::PLAYER) {
            world
                .get_nearest_player(pos, self.flee_distance, |player| {
                    EntityPredicate::ExceptCreativeOrSpectator.test(player.get_entity())
                        && player.living_entity.is_alive()
                        && (self.flee_filter)(mob, player.as_ref())
                })
                .map(|p| p as Arc<dyn EntityBase>)
        } else {
            let types = self.flee_type.map(|kind| [kind]);
            world.get_nearest_entity(
                pos,
                self.flee_distance,
                types.as_ref().map(<[_; 1]>::as_slice),
                |entity| {
                    EntityPredicate::ExceptCreativeOrSpectator.test(entity.get_entity())
                        && entity
                            .get_living_entity()
                            .is_some_and(|living| living.is_alive())
                        && (self.flee_filter)(mob, entity.as_ref())
                },
            )
        }
    }
}

impl Goal for AvoidEntityGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(target) = self.find_threat(mob) else {
            return false;
        };

        let threat_pos = target.get_entity().pos.load();
        let Some(flee_pos) =
            default_random_pos::get_pos_away(mob, HORIZONTAL_RANGE, VERTICAL_RANGE, threat_pos)
        else {
            return false;
        };

        // Give up when the escape route does not gain any distance.
        let mob_pos = mob.get_entity().pos.load();
        if threat_pos.squared_distance_to_vec(&flee_pos)
            < threat_pos.squared_distance_to_vec(&mob_pos)
        {
            return false;
        }

        self.target = Some(target);
        self.flee_pos = Some(flee_pos);
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !mob.is_navigator_idle()
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(flee_pos) = self.flee_pos {
            let mob_pos = mob.get_mob_entity().living_entity.entity.pos.load();
            let mut navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            navigator.set_progress(NavigatorGoal::new(mob_pos, flee_pos, self.slow_speed));
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(target) = &self.target {
            let mob_pos = mob.get_mob_entity().living_entity.entity.pos.load();
            let threat_pos = target.get_entity().pos.load();
            let dist_sq = mob_pos.squared_distance_to_vec(&threat_pos);
            let speed = if dist_sq < FAST_DISTANCE_SQ {
                self.fast_speed
            } else {
                self.slow_speed
            };
            let mut navigator = mob
                .get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            navigator.set_speed(speed);
        }
    }

    fn stop(&mut self, _mob: &dyn Mob) {
        self.target = None;
        self.flee_pos = None;
    }

    fn controls(&self) -> Controls {
        self.goal_control
    }
}
