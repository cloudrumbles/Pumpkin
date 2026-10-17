use crate::entity::{
    Entity, EntityBase,
    ai::goal::is_valid_attack_target,
    mob::{Mob, MobEntity},
};
use crossbeam::atomic::AtomicCell;
use pumpkin_data::{
    damage::DamageType,
    sound::{Sound, SoundCategory},
    tracked_data,
};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::math::vector3::Vector3;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicI32, Ordering},
};
use uuid::Uuid;

#[derive(Clone)]
struct Flight {
    origin: Vector3<f64>,
    destination: Option<Vector3<f64>>,
    charging: bool,
    next_attack: i64,
}

pub struct VexEntity {
    pub mob_entity: MobEntity,
    owner: AtomicCell<Option<Uuid>>,
    life_ticks: AtomicI32,
    flight: Mutex<Flight>,
}

impl VexEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let origin = entity.pos.load();
        Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            owner: AtomicCell::new(None),
            life_ticks: AtomicI32::new(-1),
            flight: Mutex::new(Flight {
                origin,
                destination: None,
                charging: false,
                next_attack: 0,
            }),
        })
    }
    pub fn set_owner(&self, owner: Uuid) {
        self.owner.store(Some(owner));
    }
    pub fn set_limited_life(&self, ticks: i32) {
        self.life_ticks.store(ticks.max(1), Ordering::Relaxed);
    }
}

impl Mob for VexEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
    fn get_mob_gravity(&self) -> f64 {
        0.0
    }
    fn get_mob_y_velocity_drag(&self) -> Option<f64> {
        Some(0.91)
    }
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.get_entity().no_physics.store(true, Ordering::Relaxed);
        if self.life_ticks.load(Ordering::Relaxed) >= 0
            && self.life_ticks.fetch_sub(1, Ordering::Relaxed) <= 1
        {
            self.life_ticks.store(20, Ordering::Relaxed);
            self.damage(self, 1.0, DamageType::STARVE);
        }
    }
    fn post_tick(&self) {
        self.get_entity().no_physics.store(false, Ordering::Relaxed);
    }
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        let world = entity.world.load();
        let now = world.get_world_age();
        let mut flight = self
            .flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mut target = self
            .mob_entity
            .get_target()
            .filter(|t| is_valid_attack_target(self, t.as_ref()));
        if target.is_none() {
            target = self
                .owner
                .load()
                .and_then(|id| world.get_entity_by_uuid(id))
                .and_then(|owner| {
                    owner
                        .get_mob()
                        .and_then(|mob| mob.get_mob_entity().get_target())
                })
                .filter(|t| is_valid_attack_target(self, t.as_ref()));
        }
        if target.is_none() {
            target = world
                .get_nearest_player(pos, 16.0, |p| {
                    is_valid_attack_target(self, p.as_ref())
                        && self.has_line_of_sight(p.get_entity())
                })
                .map(|p| p as Arc<dyn EntityBase>);
        }
        self.set_mob_target(target.clone());
        if target.is_none() && flight.charging {
            flight.charging = false;
            flight.destination = None;
        }
        if let Some(target) = &target {
            if flight.destination.is_none()
                && now >= flight.next_attack
                && pos.squared_distance_to_vec(&target.get_entity().pos.load()) > 4.0
            {
                flight.charging = true;
                flight.destination = Some(
                    target.get_entity().pos.load()
                        + Vector3::new(
                            0.0,
                            f64::from(target.get_entity().entity_dimension.load().eye_height),
                            0.0,
                        ),
                );
                world.play_sound(Sound::EntityVexCharge, SoundCategory::Hostile, &pos);
            }
            if flight.charging {
                if entity
                    .bounding_box
                    .load()
                    .intersects(&target.get_entity().bounding_box.load())
                {
                    self.mob_entity.try_attack(self, target.as_ref());
                    flight.charging = false;
                    flight.destination = None;
                    flight.next_attack = now + 20;
                } else if pos.squared_distance_to_vec(&target.get_entity().pos.load()) < 9.0 {
                    flight.destination = Some(
                        target.get_entity().pos.load()
                            + Vector3::new(
                                0.0,
                                f64::from(target.get_entity().entity_dimension.load().eye_height),
                                0.0,
                            ),
                    );
                }
            }
        }
        if flight.destination.is_none() && rand::random_range(0..7) == 0 {
            for _ in 0..3 {
                let destination = flight.origin
                    + Vector3::new(
                        rand::random_range(-7.0..=7.0),
                        rand::random_range(-5.0..=5.0),
                        rand::random_range(-7.0..=7.0),
                    );
                if world
                    .get_block(&pumpkin_util::math::position::BlockPos::floored_v(
                        destination,
                    ))
                    .is_air()
                {
                    flight.destination = Some(destination);
                    break;
                }
            }
        }
        if let Some(destination) = flight.destination {
            let delta = destination - pos;
            let distance = delta.length();
            if distance < 0.5 {
                flight.destination = None;
                flight.charging = false;
                entity.set_velocity(entity.velocity.load() * 0.5);
            } else {
                let speed = if flight.charging { 1.0 } else { 0.25 };
                entity.set_velocity(entity.velocity.load() + delta * (speed * 0.05 / distance));
                let facing = target
                    .as_ref()
                    .map_or(delta, |target| target.get_entity().pos.load() - pos);
                let yaw = -facing.x.atan2(facing.z).to_degrees() as f32;
                entity.yaw.store(yaw);
                entity.body_yaw.store(yaw);
                entity.head_yaw.store(yaw);
            }
        }
        entity.set_synced_data(
            tracked_data::vex::DATA_FLAGS_ID,
            if flight.charging { 1u8 } else { 0u8 },
        );
        *self
            .flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = flight;
    }
    fn on_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) {
        if let Some(source) = source.filter(|source| is_valid_attack_target(self, *source)) {
            let world = self.get_entity().world.load();
            if let Some(target) = world.get_entity_by_uuid(source.get_entity().entity_uuid) {
                self.set_mob_target(Some(target));
            }
        }
    }
    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if let Some(owner) = self.owner.load() {
            nbt.put_uuid("owner", owner);
        }
        let ticks = self.life_ticks.load(Ordering::Relaxed);
        if ticks >= 0 {
            nbt.put_int("life_ticks", ticks);
        }
        let origin = self
            .flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .origin;
        nbt.put(
            "bound_pos",
            NbtTag::IntArray(vec![
                origin.x.floor() as i32,
                origin.y.floor() as i32,
                origin.z.floor() as i32,
            ]),
        );
    }
    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.owner.store(nbt.get_uuid("owner"));
        self.life_ticks
            .store(nbt.get_int("life_ticks").unwrap_or(-1), Ordering::Relaxed);
        if let Some(origin) = nbt.get_int_array("bound_pos") {
            if origin.len() == 3 {
                self.flight
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .origin = Vector3::new(
                    f64::from(origin[0]),
                    f64::from(origin[1]),
                    f64::from(origin[2]),
                );
            }
        }
    }
}
