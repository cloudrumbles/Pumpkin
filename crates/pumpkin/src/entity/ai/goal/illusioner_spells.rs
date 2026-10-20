//! Illusioner casting phases, mirror invisibility, and single-target blindness.
use super::{Controls, Goal, is_valid_attack_target};
use crate::entity::{
    EntityBase,
    mob::{Mob, evoker::IllagerSpell, illusioner::IllusionerEntity},
};
use pumpkin_data::{effect::StatusEffect, potion::Effect, sound::Sound};
use pumpkin_util::Difficulty;
use std::sync::{Arc, Weak, atomic::Ordering};

pub struct IllusionerCastingGoal {
    caster: Weak<IllusionerEntity>,
}
impl IllusionerCastingGoal {
    pub const fn new(caster: Weak<IllusionerEntity>) -> Self {
        Self { caster }
    }
}
impl Goal for IllusionerCastingGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        self.caster
            .upgrade()
            .is_some_and(|caster| caster.is_casting_spell())
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob)
    }
    fn start(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
    }
    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(target) = mob.get_mob_entity().get_target() {
            mob.get_mob_entity()
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at_entity_with_range(&target, 30.0, 30.0);
        }
    }
    fn stop(&mut self, _mob: &dyn Mob) {
        if let Some(caster) = self.caster.upgrade() {
            caster.set_is_casting_spell(IllagerSpell::None);
        }
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IllusionerCast {
    Mirror,
    Blindness,
}

pub struct IllusionerSpellGoal {
    caster: Weak<IllusionerEntity>,
    kind: IllusionerCast,
    warmup: i32,
    next_cast: i32,
    last_target: Option<i32>,
    target: Option<Arc<dyn EntityBase>>,
}

impl IllusionerSpellGoal {
    pub const fn new(caster: Weak<IllusionerEntity>, kind: IllusionerCast) -> Self {
        Self {
            caster,
            kind,
            warmup: 0,
            next_cast: 0,
            last_target: None,
            target: None,
        }
    }
}

impl Goal for IllusionerSpellGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(caster) = self.caster.upgrade() else {
            return false;
        };
        let Some(target) = caster.mob_entity.get_target() else {
            return false;
        };
        if !is_valid_attack_target(mob, target.as_ref())
            || caster.is_casting_spell()
            || caster.get_entity().tick_count.load(Ordering::Relaxed) < self.next_cast
        {
            return false;
        }
        match self.kind {
            IllusionerCast::Mirror
                if caster
                    .mob_entity
                    .living_entity
                    .has_effect(&StatusEffect::INVISIBILITY) =>
            {
                return false;
            }
            IllusionerCast::Blindness
                if self.last_target == Some(target.get_entity().entity_id)
                    || caster
                        .get_entity()
                        .world
                        .load()
                        .level_info
                        .load()
                        .difficulty
                        != Difficulty::Hard =>
            {
                return false;
            }
            _ => {}
        }
        self.target = Some(target);
        true
    }
    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        self.warmup > 0
            && self
                .target
                .as_ref()
                .is_some_and(|target| target.get_entity().is_alive())
    }
    fn start(&mut self, mob: &dyn Mob) {
        let Some(caster) = self.caster.upgrade() else {
            return;
        };
        self.warmup = 20;
        let entity = caster.get_entity();
        self.next_cast = entity.tick_count.load(Ordering::Relaxed)
            + match self.kind {
                IllusionerCast::Mirror => 340,
                IllusionerCast::Blindness => 180,
            };
        caster.set_spell_casting_time(40);
        match self.kind {
            IllusionerCast::Mirror => {
                caster.set_is_casting_spell(IllagerSpell::Disappear);
                entity.play_sound(Sound::EntityIllusionerPrepareMirror);
            }
            IllusionerCast::Blindness => {
                caster.set_is_casting_spell(IllagerSpell::Blindness);
                entity.play_sound(Sound::EntityIllusionerPrepareBlindness);
                self.last_target = self
                    .target
                    .as_ref()
                    .map(|target| target.get_entity().entity_id);
            }
        }
        // SpellcasterIllager stops moving when the casting goal takes over.
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
    }
    fn tick(&mut self, _mob: &dyn Mob) {
        self.warmup -= 1;
        if self.warmup != 0 {
            return;
        }
        let Some(caster) = self.caster.upgrade() else {
            return;
        };
        let effect = match self.kind {
            IllusionerCast::Mirror => Effect {
                effect_type: &StatusEffect::INVISIBILITY,
                duration: 1200,
                amplifier: 0,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: false,
            },
            IllusionerCast::Blindness => Effect {
                effect_type: &StatusEffect::BLINDNESS,
                duration: 400,
                amplifier: 0,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: false,
            },
        };
        match self.kind {
            IllusionerCast::Mirror => caster.mob_entity.living_entity.add_effect(effect),
            IllusionerCast::Blindness => {
                if let Some(target) = self
                    .target
                    .as_ref()
                    .and_then(|target| target.get_living_entity())
                {
                    target.add_effect(effect);
                }
            }
        }
        caster
            .get_entity()
            .play_sound(Sound::EntityIllusionerCastSpell);
    }
    fn stop(&mut self, _mob: &dyn Mob) {
        self.target = None;
    }
    fn controls(&self) -> Controls {
        Controls::empty()
    }
}
