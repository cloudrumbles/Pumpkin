//! Reach-based spear combat: approach, engage, charge and reposition between uses.
//! Vanilla's `SpearUseGoal` requires a kinetic weapon in the main hand.
use pumpkin_data::{
    data_component_impl::EquipmentSlot,
    tag::{self, Taggable},
};
use pumpkin_util::math::vector3::Vector3;

use super::{Controls, Goal, is_valid_attack_target};
use crate::entity::{ai::pathfinder::NavigatorGoal, mob::Mob};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Approach,
    Windup,
    Charge,
    Retreat,
}

pub struct SpearUseGoal {
    phase: Phase,
    ticks: i32,
    cooldown: i32,
}

impl SpearUseGoal {
    pub const fn new() -> Self {
        Self {
            phase: Phase::Approach,
            ticks: 0,
            cooldown: 0,
        }
    }

    pub fn has_spear(mob: &dyn Mob) -> bool {
        let equipment = mob
            .get_mob_entity()
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        equipment
            .get(&EquipmentSlot::MAIN_HAND)
            .item
            .has_tag(&tag::Item::MINECRAFT_SPEARS)
    }
    fn navigate(mob: &dyn Mob, destination: Vector3<f64>, speed: f64) {
        let entity = mob.get_entity();
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal::new(entity.pos.load(), destination, speed));
    }
}

impl Goal for SpearUseGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        Self::has_spear(mob)
            && mob
                .get_mob_entity()
                .get_target()
                .is_some_and(|target| is_valid_attack_target(mob, target.as_ref()))
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob)
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.phase = Phase::Approach;
        self.ticks = 0;
        self.cooldown = 0;
        mob.get_mob_entity().set_attacking(true);
    }
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        let origin = mob.get_entity().pos.load();
        let destination = target.get_entity().pos.load();
        let delta = destination - origin;
        let distance = delta.length();
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(&target, 30.0, 30.0);
        self.ticks += 1;
        match self.phase {
            Phase::Approach => {
                if distance > 10.0 {
                    Self::navigate(mob, destination, 1.0);
                } else {
                    self.phase = Phase::Windup;
                    self.ticks = 0;
                    mob.get_mob_entity()
                        .navigator
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .stop();
                }
            }
            Phase::Windup => {
                // Allow an observable winding-up phase before the forward rush.
                if self.ticks >= 10 {
                    self.phase = Phase::Charge;
                    self.ticks = 0;
                }
            }
            Phase::Charge => {
                Self::navigate(mob, destination, 1.0);
                if distance <= 2.0 {
                    mob.get_mob_entity().living_entity.swing_hand();
                    mob.get_mob_entity().try_attack(mob, target.as_ref());
                    self.phase = Phase::Retreat;
                    self.ticks = 0;
                    self.cooldown = 20;
                } else if self.ticks > 60 {
                    self.phase = Phase::Retreat;
                    self.ticks = 0;
                    self.cooldown = 20;
                }
            }
            Phase::Retreat => {
                if self.ticks < self.cooldown && distance < 7.0 {
                    let away = if distance > 0.001 {
                        origin - delta * (6.0 / distance)
                    } else {
                        origin + Vector3::new(6.0, 0.0, 0.0)
                    };
                    Self::navigate(mob, away, 1.0);
                } else if self.ticks >= self.cooldown {
                    self.phase = Phase::Approach;
                    self.ticks = 0;
                }
            }
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        mob.get_mob_entity().set_attacking(false);
        self.phase = Phase::Approach;
        self.ticks = 0;
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}
