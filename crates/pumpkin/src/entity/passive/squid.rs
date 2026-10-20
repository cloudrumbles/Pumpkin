use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::aquatic::aquatic_gravity,
    mob::{Mob, MobEntity},
};
use pumpkin_data::{
    damage::DamageType, entity::EntityType, particle::Particle, sound::Sound, tracked_data,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicI32, Ordering},
};

struct Swimming {
    direction: Vector3<f64>,
    velocity: Vector3<f64>,
    phase: f64,
    phase_speed: f64,
    attacker: Option<Weak<dyn EntityBase>>,
}

pub struct SquidEntity {
    pub mob_entity: MobEntity,
    swimming: Mutex<Swimming>,
    ageable_data: AgeableData,
    dark_ticks: AtomicI32,
}

impl SquidEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            ageable_data: AgeableData::default(),
            dark_ticks: AtomicI32::new(0),
            swimming: Mutex::new(Swimming {
                direction: Vector3::new(0.0, 0.0, 0.0),
                velocity: Vector3::new(0.0, 0.0, 0.0),
                phase: 0.0,
                phase_speed: 0.2 / rand::random_range(1.0..2.0),
                attacker: None,
            }),
        })
    }
    fn is_glow(&self) -> bool {
        self.get_entity().entity_type.id == EntityType::GLOW_SQUID.id
    }
}

impl AgeableMob for SquidEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Mob for SquidEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }
    fn get_mob_gravity(&self) -> f64 {
        aquatic_gravity(&self.mob_entity)
    }
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ageable_ai_step();
        if self.is_glow() {
            let dark = self.dark_ticks.load(Ordering::Relaxed);
            if dark > 0 {
                self.dark_ticks.store(dark - 1, Ordering::Relaxed);
                self.get_entity().set_synced_data(
                    tracked_data::glow_squid::DATA_DARK_TICKS_REMAINING,
                    VarInt(dark - 1),
                );
            }
            let entity = self.get_entity();
            entity.world.load().spawn_particle(
                entity.pos.load(),
                Vector3::new(0.5, 0.5, 0.5),
                0.0,
                1,
                Particle::Glow,
            );
        }
    }
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        let world = entity.world.load();
        let mut swim = self
            .swimming
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        swim.phase += swim.phase_speed;
        if swim.phase > std::f64::consts::TAU {
            swim.phase -= std::f64::consts::TAU;
            if rand::random_range(0..10) == 0 {
                swim.phase_speed = 0.2 / rand::random_range(1.0..2.0);
            }
        }
        if !entity.touching_water.load(Ordering::Relaxed) {
            entity
                .velocity
                .store(Vector3::new(0.0, entity.velocity.load().y, 0.0));
            entity
                .pitch
                .store(entity.pitch.load() + (-90.0 - entity.pitch.load()) * 0.02);
            swim.direction = Vector3::new(0.0, 0.0, 0.0);
            return;
        }
        if swim.direction.length_squared() < 1e-5
            || entity.horizontal_collision.load(Ordering::Relaxed)
            || rand::random_range(0..50) == 0
        {
            let angle = rand::random_range(0.0..std::f64::consts::TAU);
            swim.direction = Vector3::new(
                angle.cos() * 0.2,
                rand::random_range(-0.1..0.1),
                angle.sin() * 0.2,
            );
        }
        if let Some(attacker) = swim.attacker.as_ref().and_then(Weak::upgrade) {
            let away = pos - attacker.get_entity().pos.load();
            let distance = away.length();
            if attacker.get_entity().is_alive() && distance > 0.0 && distance < 10.0 {
                let candidate = pos + away;
                let water = world
                    .get_fluid(&BlockPos::floored_v(candidate))
                    .matches_type(&pumpkin_data::fluid::Fluid::WATER);
                if water || world.get_block(&BlockPos::floored_v(candidate)).is_air() {
                    let speed = (3.0 - ((distance - 5.0) / 5.0).max(0.0)) / 20.0;
                    swim.direction = away.normalize() * speed;
                    if !water {
                        swim.direction.y = 0.0;
                    }
                }
            } else {
                swim.attacker = None;
            }
        }
        if swim.phase < std::f64::consts::PI {
            if swim.phase / std::f64::consts::PI > 0.75 {
                swim.velocity = swim.direction;
            }
        } else {
            swim.velocity = swim.velocity * 0.9;
        }
        entity.velocity.store(swim.velocity);
        let yaw = -swim.velocity.x.atan2(swim.velocity.z).to_degrees() as f32;
        let horizontal = swim.velocity.horizontal_length();
        entity
            .yaw
            .store(entity.yaw.load() + (yaw - entity.yaw.load()) * 0.1);
        entity.body_yaw.store(entity.yaw.load());
        entity
            .pitch
            .store((-horizontal.atan2(swim.velocity.y).to_degrees()) as f32);
    }
    fn on_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) {
        let entity = self.get_entity();
        let world = entity.world.load();
        if let Some(attacker) =
            source.and_then(|source| world.get_entity_by_uuid(source.get_entity().entity_uuid))
        {
            self.swimming
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .attacker = Some(Arc::downgrade(&attacker));
            world.spawn_particle(
                entity.pos.load() + Vector3::new(0.0, 0.5, 0.0),
                Vector3::new(0.3, 0.3, 0.3),
                0.1,
                30,
                if self.is_glow() {
                    Particle::GlowSquidInk
                } else {
                    Particle::SquidInk
                },
            );
            entity.play_sound(if self.is_glow() {
                Sound::EntityGlowSquidSquirt
            } else {
                Sound::EntitySquidSquirt
            });
        }
        if self.is_glow() {
            self.dark_ticks.store(100, Ordering::Relaxed);
            entity.set_synced_data(
                tracked_data::glow_squid::DATA_DARK_TICKS_REMAINING,
                VarInt(100),
            );
        }
    }
    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        if self.is_glow() {
            nbt.put_int(
                "DarkTicksRemaining",
                self.dark_ticks.load(Ordering::Relaxed),
            );
        }
    }
    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        if self.is_glow() {
            self.dark_ticks.store(
                nbt.get_int("DarkTicksRemaining").unwrap_or(0).max(0),
                Ordering::Relaxed,
            );
        }
    }
}
