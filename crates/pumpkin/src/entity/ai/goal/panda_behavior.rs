//! Gene-specific panda play, resting, and defense goals.
use std::sync::atomic::Ordering;

use super::{Controls, Goal};
use crate::entity::{
    mob::Mob,
    passive::panda::{PandaEntity, PandaGene},
};
use pumpkin_data::entity::EntityType;

use rand::RngExt;

fn panda(mob: &dyn Mob) -> Option<&PandaEntity> {
    mob.cast_any().downcast_ref::<PandaEntity>()
}

pub fn can_act(mob: &dyn Mob) -> bool {
    panda(mob).is_some_and(|p| {
        !p.is_sitting()
            && !p.is_on_back()
            && !p.is_rolling()
            && !p.is_sneezing()
            && p.unhappy_counter.load(Ordering::Relaxed) == 0
    })
}

pub fn can_breed(mob: &dyn Mob) -> bool {
    let Some(p) = panda(mob) else {
        return false;
    };
    if p.unhappy_counter.load(Ordering::Relaxed) > 0 {
        return false;
    }
    let entity = mob.get_entity();
    let current = entity.block_pos.load();
    let world = entity.world.load();
    for dx in -7..=7 {
        for dz in -7..=7 {
            for dy in 0..=2 {
                let pos = pumpkin_util::math::position::BlockPos::new(
                    current.0.x + dx,
                    current.0.y + dy,
                    current.0.z + dz,
                );
                if world.get_block(&pos) == &pumpkin_data::Block::BAMBOO {
                    return true;
                }
            }
        }
    }
    false
}

pub struct PandaSneezeGoal;
impl Goal for PandaSneezeGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(p) = panda(mob) else {
            return false;
        };
        let chance = if p.get_phenotype() == PandaGene::Weak {
            500
        } else {
            6000
        };
        mob.get_entity().age.load(Ordering::Relaxed) < 0
            && can_act(mob)
            && mob.get_random().random_range(0..chance) == 1
    }
    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }
    fn start(&mut self, mob: &dyn Mob) {
        if let Some(p) = panda(mob) {
            p.set_sneezing(true);
            p.sneeze_counter.store(0, Ordering::Relaxed);
        }
    }
}

pub struct PandaRollGoal;
impl Goal for PandaRollGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(p) = panda(mob) else {
            return false;
        };
        let chance = if p.get_phenotype() == PandaGene::Playful {
            60
        } else {
            500
        };
        (mob.get_entity().age.load(Ordering::Relaxed) < 0
            || p.get_phenotype() == PandaGene::Playful)
            && can_act(mob)
            && mob.get_entity().on_ground.load(Ordering::Relaxed)
            && mob.get_random().random_range(0..chance) == 1
    }
    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }
    fn start(&mut self, mob: &dyn Mob) {
        if let Some(p) = panda(mob) {
            p.set_rolling(true);
            p.roll_counter.store(0, Ordering::Relaxed);
        }
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK | Controls::JUMP
    }
}

pub struct PandaSitGoal {
    until: i64,
    cooldown: i64,
}
impl PandaSitGoal {
    pub const fn new() -> Self {
        Self {
            until: 0,
            cooldown: 0,
        }
    }
}
impl Goal for PandaSitGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        entity.age.load(Ordering::Relaxed) >= 0
            && entity.world.load().get_world_age() >= self.cooldown
            && !entity.touching_water.load(Ordering::Relaxed)
            && can_act(mob)
            && mob.get_random().random_range(0..400) == 1
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(p) = panda(mob) else {
            return false;
        };
        let entity = mob.get_entity();
        p.is_sitting()
            && p.get_mob_entity().get_target().is_none()
            && !entity.touching_water.load(Ordering::Relaxed)
            && entity.world.load().get_world_age() < self.until
    }
    fn start(&mut self, mob: &dyn Mob) {
        if let Some(p) = panda(mob) {
            p.set_sitting(true);
            let duration = if p.get_phenotype() == PandaGene::Lazy {
                700
            } else {
                200
            };
            self.until = mob.get_entity().world.load().get_world_age() + duration;
            p.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(p) = panda(mob) {
            p.set_sitting(false);
            p.eat_counter.store(0, Ordering::Relaxed);
        }
        self.cooldown = mob.get_entity().world.load().get_world_age() + 200;
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

pub struct PandaLieBackGoal {
    until: i64,
    cooldown: i64,
}
impl PandaLieBackGoal {
    pub const fn new() -> Self {
        Self {
            until: 0,
            cooldown: 0,
        }
    }
}
impl Goal for PandaLieBackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        panda(mob).is_some_and(|p| p.get_phenotype() == PandaGene::Lazy)
            && can_act(mob)
            && mob.get_entity().world.load().get_world_age() > self.cooldown
            && mob.get_random().random_range(0..400) == 1
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        panda(mob).is_some_and(|p| p.is_on_back())
            && mob.get_entity().world.load().get_world_age() < self.until
            && !mob.get_entity().touching_water.load(Ordering::Relaxed)
            && mob.get_mob_entity().get_target().is_none()
    }
    fn start(&mut self, mob: &dyn Mob) {
        if let Some(p) = panda(mob) {
            p.set_on_back(true);
        }
        self.until = mob.get_entity().world.load().get_world_age() + 160;
    }
    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(p) = panda(mob) {
            p.set_on_back(false);
        }
        self.cooldown = mob.get_entity().world.load().get_world_age() + 500;
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

pub fn avoid_worried(mob: &dyn Mob, other: &dyn crate::entity::EntityBase) -> bool {
    panda(mob).is_some_and(|p| p.get_phenotype() == PandaGene::Worried)
        && can_act(mob)
        && (other.get_entity().entity_type == &EntityType::PLAYER
            || other.get_entity().entity_type == &EntityType::WOLF)
}

pub fn panda_attack_handler(
    mob: &dyn Mob,
    target: &dyn crate::entity::EntityBase,
    goal: &mut super::melee_attack::MeleeAttackGoal,
) {
    let Some(p) = panda(mob) else {
        return;
    };
    if !can_act(mob) || !goal.can_perform_attack(mob, target) {
        return;
    }
    goal.perform_attack(mob, target);
    mob.get_entity()
        .play_sound(pumpkin_data::sound::Sound::EntityPandaBite);
    // A non-aggressive panda bites in retaliation once, then withdraws.
    if p.get_phenotype() != PandaGene::Aggressive {
        mob.set_mob_target(None);
    }
}
