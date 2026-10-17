use crate::entity::{
    Entity, EntityBase,
    ai::goal::aquatic::{RandomSwimGoal, aquatic_gravity, configure_swimming, flop},
    mob::{Mob, MobEntity},
    player::Player,
};
use pumpkin_data::{
    damage::DamageType,
    effect::StatusEffect,
    potion::Effect,
    sound::Sound,
    tag::{self, Taggable},
    tracked_data,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::boundingbox::{BoundingBox, EntityDimensions};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

pub struct PufferfishEntity {
    pub mob_entity: MobEntity,
    puff: AtomicI32,
    inflate_ticks: AtomicI32,
    deflate_ticks: AtomicI32,
    from_bucket: AtomicBool,
}

impl PufferfishEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        configure_swimming(&mob_entity);
        mob_entity
            .goals_selector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .add_goal(4, Box::new(RandomSwimGoal::fish(0)));
        let fish = Arc::new(Self {
            mob_entity,
            puff: AtomicI32::new(0),
            inflate_ticks: AtomicI32::new(0),
            deflate_ticks: AtomicI32::new(0),
            from_bucket: AtomicBool::new(false),
        });
        fish.set_puff_state(0);
        fish
    }

    fn set_puff_state(&self, state: i32) {
        let state = state.clamp(0, 2);
        self.puff.store(state, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(tracked_data::pufferfish::PUFF_STATE, VarInt(state));
        let scale = match state {
            0 => 0.5,
            1 => 0.7,
            _ => 1.0,
        };
        let dimensions = EntityDimensions {
            width: entity.entity_type.dimension[0] * scale,
            height: entity.entity_type.dimension[1] * scale,
            eye_height: entity.entity_type.eye_height * scale,
        };
        entity.entity_dimension.store(dimensions);
        let pos = entity.pos.load();
        entity
            .bounding_box
            .store(BoundingBox::new_from_pos(pos.x, pos.y, pos.z, &dimensions));
    }

    fn is_scary(&self, target: &dyn EntityBase) -> bool {
        target.get_entity().entity_id != self.get_entity().entity_id
            && target.get_living_entity().is_some()
            && target.get_entity().is_alive()
            && !target.is_spectator()
            && !target.get_player().is_some_and(|p| p.is_creative())
            && !target
                .get_entity()
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_NOT_SCARY_FOR_PUFFERFISH)
    }

    fn sting(&self, target: &dyn EntityBase) {
        let puff = self.puff.load(Ordering::Relaxed);
        if puff == 0
            || !self.is_scary(target)
            || !target
                .get_living_entity()
                .is_some_and(|living| self.can_attack(living))
        {
            return;
        }
        if target.damage_with_context(
            target,
            (1 + puff) as f32,
            DamageType::MOB_ATTACK,
            Some(self.get_entity().pos.load()),
            Some(self),
            Some(self),
        ) {
            if let Some(living) = target.get_living_entity() {
                living.add_effect(Effect {
                    effect_type: &StatusEffect::POISON,
                    duration: 60 * puff,
                    amplifier: 0,
                    ambient: false,
                    show_particles: true,
                    show_icon: true,
                    blend: true,
                });
            }
            self.get_entity().play_sound(Sound::EntityPufferFishSting);
        }
    }
}

impl Mob for PufferfishEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
    fn get_mob_gravity(&self) -> f64 {
        aquatic_gravity(&self.mob_entity)
    }
    fn requires_custom_persistence(&self) -> bool {
        self.from_bucket.load(Ordering::Relaxed)
    }
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let scary = world
            .get_all_at_box(&entity.bounding_box.load().expand(2.0, 2.0, 2.0))
            .iter()
            .any(|target| self.is_scary(target.as_ref()));
        let state = self.puff.load(Ordering::Relaxed);
        if scary {
            self.deflate_ticks.store(0, Ordering::Relaxed);
            let ticks = self.inflate_ticks.fetch_add(1, Ordering::Relaxed) + 1;
            if state == 0 || (ticks > 40 && state == 1) {
                self.set_puff_state(state + 1);
                entity.play_sound(Sound::EntityPufferFishBlowUp);
            }
        } else {
            self.inflate_ticks.store(0, Ordering::Relaxed);
            let ticks = self.deflate_ticks.fetch_add(1, Ordering::Relaxed);
            if (ticks > 60 && state == 2) || (ticks > 100 && state == 1) {
                self.set_puff_state(state - 1);
                entity.play_sound(Sound::EntityPufferFishBlowOut);
            }
        }
        flop(self);
    }
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        if self.puff.load(Ordering::Relaxed) > 0 {
            let entity = self.get_entity();
            for target in entity
                .world
                .load()
                .get_entities_at_box(&entity.bounding_box.load().expand(0.3, 0.3, 0.3))
            {
                self.sting(target.as_ref());
            }
        }
    }
    fn mob_player_collision(&self, player: &Arc<Player>) {
        self.sting(player.as_ref());
    }
    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("PuffState", self.puff.load(Ordering::Relaxed));
        nbt.put_bool("FromBucket", self.from_bucket.load(Ordering::Relaxed));
    }
    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_puff_state(nbt.get_int("PuffState").unwrap_or(0));
        self.from_bucket.store(
            nbt.get_bool("FromBucket").unwrap_or(false),
            Ordering::Relaxed,
        );
    }
}
