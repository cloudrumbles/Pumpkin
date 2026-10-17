use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};

use pumpkin_data::{damage::DamageType, effect::StatusEffect, potion::Effect};
use pumpkin_protocol::java::client::play::{CGameEvent, GameEvent};

use super::guardian::{guardian_thorns, register_guardian_ai, update_guardian_movement};
use crate::entity::{
    Entity, EntityBase,
    ai::goal::aquatic,
    mob::{Mob, MobEntity},
};

pub struct ElderGuardianEntity {
    pub mob_entity: MobEntity,
    tick_count: AtomicI32,
}

impl ElderGuardianEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let guardian = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            tick_count: AtomicI32::new(0),
        });
        let mob: Arc<dyn Mob> = guardian.clone();
        register_guardian_ai(&guardian.mob_entity, Arc::downgrade(&mob), true);
        guardian
    }
}

impl Mob for ElderGuardianEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
    fn get_mob_gravity(&self) -> f64 {
        aquatic::aquatic_gravity(&self.mob_entity)
    }
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        update_guardian_movement(self);
        let entity = self.get_entity();
        let ticks = self.tick_count.fetch_add(1, Ordering::Relaxed) + 1;
        if (ticks + entity.entity_id).rem_euclid(1200) != 0 {
            return;
        }
        let world = entity.world.load();
        for player in world.get_nearby_players(entity.pos.load(), 50.0) {
            if !matches!(
                player.gamemode.load(),
                pumpkin_util::GameMode::Survival | pumpkin_util::GameMode::Adventure
            ) {
                continue;
            }
            let existing = player
                .living_entity
                .get_effect(&StatusEffect::MINING_FATIGUE);
            if existing.is_some_and(|effect| {
                effect.amplifier > 2
                    || (effect.amplifier == 2 && (effect.duration < 0 || effect.duration >= 1200))
            }) {
                continue;
            }
            player.try_send_client_packet(&CGameEvent::new(
                GameEvent::PlayElderGuardianMobAppearance,
                1.0,
            ));
            let fatigue = Effect {
                effect_type: &StatusEffect::MINING_FATIGUE,
                duration: 6000,
                amplifier: 2,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: false,
            };
            player.send_effect(&fatigue);
            player.living_entity.add_effect(fatigue);
        }
    }
    fn on_damage(&self, damage_type: DamageType, source: Option<&dyn EntityBase>) {
        guardian_thorns(self, damage_type, source);
    }
}
