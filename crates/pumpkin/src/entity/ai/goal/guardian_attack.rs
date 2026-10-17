use pumpkin_data::{
    damage::DamageType,
    entity::{EntityStatus, EntityType},
    tracked_data,
};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::{Difficulty, math::vector3::Vector3};

use super::{Controls, Goal};
use crate::entity::mob::Mob;

/// Guardian beams warm up for ten ticks, then charge for 80 ticks (60 for elders).
/// Losing sight, a dead target, or a target entering creative cancels the beam.
pub struct GuardianAttackGoal {
    attack_time: i32,
    elder: bool,
}

impl GuardianAttackGoal {
    pub const fn new(elder: bool) -> Self {
        Self {
            attack_time: -10,
            elder,
        }
    }

    fn clear_beam(mob: &dyn Mob) {
        mob.get_entity()
            .set_synced_data(tracked_data::guardian::DATA_ID_ATTACK_TARGET, VarInt(0));
    }
}

impl Goal for GuardianAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .get_target()
            .is_some_and(|target| super::is_valid_attack_target(mob, target.as_ref()))
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob)
            && (self.elder
                || mob.get_mob_entity().get_target().is_some_and(|target| {
                    mob.get_entity()
                        .pos
                        .load()
                        .squared_distance_to_vec(&target.get_entity().pos.load())
                        > 9.0
                }))
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.attack_time = -10;
        Self::clear_beam(mob);
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        mob.get_mob_entity()
            .living_entity
            .movement_input
            .store(Vector3::new(0.0, 0.0, 0.0));
        mob.get_entity()
            .set_synced_data(tracked_data::guardian::DATA_ID_MOVING, false);
    }
    fn stop(&mut self, mob: &dyn Mob) {
        Self::clear_beam(mob);
        mob.set_mob_target(None);
    }
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        if !super::is_valid_attack_target(mob, target.as_ref())
            || !mob.has_line_of_sight(target.get_entity())
        {
            self.stop(mob);
            return;
        }
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(&target, 90.0, 90.0);
        self.attack_time += 1;
        let entity = mob.get_entity();
        if self.attack_time == 0 {
            entity.set_synced_data(
                tracked_data::guardian::DATA_ID_ATTACK_TARGET,
                VarInt(target.get_entity().entity_id),
            );
            entity
                .world
                .load()
                .send_entity_status(entity, EntityStatus::GuardianAttackSound, None);
        } else if self.attack_time >= if self.elder { 60 } else { 80 } {
            let mut magic = if self.elder { 3.0 } else { 1.0 };
            if entity.world.load().level_info.load().difficulty == Difficulty::Hard {
                magic += 2.0;
            }
            target.damage_with_context(
                target.as_ref(),
                magic,
                DamageType::INDIRECT_MAGIC,
                Some(entity.pos.load()),
                Some(entity),
                Some(mob),
            );
            mob.get_mob_entity().try_attack(mob, target.as_ref());
            self.stop(mob);
        }
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

pub fn is_guardian_prey(entity_type: &'static EntityType) -> bool {
    [
        EntityType::PLAYER.id,
        EntityType::SQUID.id,
        EntityType::GLOW_SQUID.id,
        EntityType::AXOLOTL.id,
    ]
    .contains(&entity_type.id)
}
