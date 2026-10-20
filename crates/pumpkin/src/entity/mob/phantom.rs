use crate::entity::{
    Entity, EntityBase,
    ai::goal::is_valid_attack_target,
    mob::{Mob, MobEntity},
};
use pumpkin_data::{
    attributes::Attributes,
    entity::EntityType,
    sound::{Sound, SoundCategory},
    tracked_data,
};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::{vector3::Vector3, wrap_degrees};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicI32, Ordering},
};

#[derive(Clone)]
struct Flight {
    anchor: Vector3<f64>,
    destination: Vector3<f64>,
    angle: f64,
    radius: f64,
    height: f64,
    clockwise: f64,
    speed: f64,
    swooping: bool,
    next_scan: i64,
    next_sweep: i64,
    next_cat_scan: i64,
    target_id: Option<i32>,
}

pub struct PhantomEntity {
    pub mob_entity: MobEntity,
    flight: Mutex<Flight>,
    size: AtomicI32,
}

impl PhantomEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let anchor = entity.pos.load() + Vector3::new(0.0, 5.0, 0.0);
        Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            size: AtomicI32::new(0),
            flight: Mutex::new(Flight {
                anchor,
                destination: anchor,
                angle: 0.0,
                radius: rand::random_range(5.0..15.0),
                height: rand::random_range(-4.0..5.0),
                clockwise: if rand::random() { 1.0 } else { -1.0 },
                speed: 0.1,
                swooping: false,
                next_scan: 0,
                next_sweep: 0,
                next_cat_scan: 0,
                target_id: None,
            }),
        })
    }

    fn steer(&self, flight: &mut Flight) {
        let entity = self.get_entity();
        let delta = flight.destination - entity.pos.load();
        let horizontal = delta.x.hypot(delta.z);
        if horizontal < 0.00001 {
            return;
        }
        let scale = 1.0 - (delta.y * 0.7).abs() / horizontal;
        let delta = Vector3::new(delta.x * scale, delta.y, delta.z * scale);
        let length = delta.length();
        if length < 0.00001 {
            return;
        }
        let previous_yaw = entity.yaw.load();
        let desired_yaw = delta.z.atan2(delta.x).to_degrees() as f32 - 90.0;
        let yaw = previous_yaw + wrap_degrees(desired_yaw - previous_yaw).clamp(-4.0, 4.0);
        entity.yaw.store(yaw);
        entity.body_yaw.store(yaw);
        entity.head_yaw.store(yaw);
        if wrap_degrees(yaw - previous_yaw).abs() < 3.0 {
            flight.speed = (flight.speed + 0.005 * (1.8 / flight.speed)).min(1.8);
        } else {
            flight.speed += (0.2 - flight.speed).clamp(-0.025, 0.025);
        }
        let pitch = delta.y.atan2(delta.x.hypot(delta.z));
        entity.pitch.store(pitch.to_degrees() as f32);
        let heading = f64::from(yaw + 90.0).to_radians();
        let desired = Vector3::new(
            flight.speed * heading.cos() * (delta.x / length).abs(),
            flight.speed * pitch.sin() * (delta.y / length).abs(),
            flight.speed * heading.sin() * (delta.z / length).abs(),
        );
        let velocity = entity.velocity.load();
        entity.set_velocity(velocity + (desired - velocity) * 0.2);
    }
}

impl Mob for PhantomEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
    fn get_mob_gravity(&self) -> f64 {
        0.0
    }
    fn get_mob_y_velocity_drag(&self) -> Option<f64> {
        Some(0.91)
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();
        let now = world.get_world_age();
        let mut flight = self
            .flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mut target = self
            .mob_entity
            .get_target()
            .filter(|target| is_valid_attack_target(self, target.as_ref()));
        if target.is_none() && now >= flight.next_scan {
            flight.next_scan = now + 60;
            let mut players = world.get_nearby_players(pos, 64.0);
            players.sort_by(|a, b| {
                b.get_entity()
                    .pos
                    .load()
                    .y
                    .total_cmp(&a.get_entity().pos.load().y)
            });
            target = players
                .into_iter()
                .find(|p| {
                    let delta = p.get_entity().pos.load() - pos;
                    delta.x.abs() <= 16.0
                        && delta.z.abs() <= 16.0
                        && is_valid_attack_target(self, p.as_ref())
                        && self.has_line_of_sight(p.get_entity())
                })
                .map(|p| p as Arc<dyn EntityBase>);
        }
        self.set_mob_target(target.clone());
        if let Some(target) = &target {
            if flight.target_id != Some(target.get_entity().entity_id) {
                flight.target_id = Some(target.get_entity().entity_id);
                flight.next_sweep = now + 10;
                flight.swooping = false;
                flight.anchor = target.get_entity().pos.load()
                    + Vector3::new(0.0, rand::random_range(20.0..40.0), 0.0);
                flight.anchor.y = flight.anchor.y.max(f64::from(world.sea_level) + 1.0);
            }
            if !flight.swooping && now >= flight.next_sweep {
                flight.swooping = true;
                flight.next_sweep = now + rand::random_range(160..240);
                world.play_sound(Sound::EntityPhantomSwoop, SoundCategory::Hostile, &pos);
            }
        } else {
            flight.swooping = false;
            flight.target_id = None;
        }
        if flight.swooping && now >= flight.next_cat_scan {
            flight.next_cat_scan = now + 20;
            if let Some(cat) =
                world.get_nearest_entity(pos, 16.0, Some(&[&EntityType::CAT]), |cat| {
                    cat.get_entity().is_alive()
                })
            {
                world.play_sound(
                    Sound::EntityCatHiss,
                    SoundCategory::Neutral,
                    &cat.get_entity().pos.load(),
                );
                flight.swooping = false;
                flight.target_id = None;
                target = None;
                self.set_mob_target(None);
                flight.next_scan = now + 60;
            }
        }
        if flight.swooping {
            if let Some(target) = &target {
                flight.destination = target.get_entity().pos.load()
                    + Vector3::new(0.0, f64::from(target.get_entity().height()) * 0.5, 0.0);
                if entity
                    .bounding_box
                    .load()
                    .expand(0.2, 0.2, 0.2)
                    .intersects(&target.get_entity().bounding_box.load())
                {
                    self.mob_entity.try_attack(self, target.as_ref());
                    world.play_sound(Sound::EntityPhantomBite, SoundCategory::Hostile, &pos);
                    flight.swooping = false;
                } else if entity.horizontal_collision.load(Ordering::Relaxed)
                    || self
                        .mob_entity
                        .living_entity
                        .hurt_cooldown
                        .load(Ordering::Relaxed)
                        > 0
                {
                    flight.swooping = false;
                }
            }
        }
        if !flight.swooping {
            if pos.squared_distance_to_vec(&flight.destination) < 4.0
                || entity.horizontal_collision.load(Ordering::Relaxed)
                || rand::random_range(0..350) == 0
            {
                flight.angle += flight.clockwise * 15.0_f64.to_radians();
                flight.destination = flight.anchor
                    + Vector3::new(
                        flight.radius * flight.angle.cos(),
                        -4.0 + flight.height,
                        flight.radius * flight.angle.sin(),
                    );
                if entity.horizontal_collision.load(Ordering::Relaxed) {
                    flight.destination.y = pos.y + 3.0;
                    flight.speed = 0.1;
                }
            }
        }
        self.steer(&mut flight);
        *self
            .flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = flight;
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        let anchor = self
            .flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .anchor;
        nbt.put(
            "anchor_pos",
            NbtTag::IntArray(vec![
                anchor.x.floor() as i32,
                anchor.y.floor() as i32,
                anchor.z.floor() as i32,
            ]),
        );
        nbt.put_int("size", self.size.load(Ordering::Relaxed));
    }
    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(anchor) = nbt.get_int_array("anchor_pos") {
            if anchor.len() == 3 {
                self.flight
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .anchor = Vector3::new(
                    f64::from(anchor[0]),
                    f64::from(anchor[1]),
                    f64::from(anchor[2]),
                );
            }
        }
        let size = nbt
            .get_int("size")
            .or_else(|| nbt.get_int("Size"))
            .unwrap_or(0)
            .clamp(0, 64);
        self.size.store(size, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(tracked_data::phantom::ID_SIZE, VarInt(size));
        if let Some(damage) = self
            .mob_entity
            .living_entity
            .attributes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(&Attributes::ATTACK_DAMAGE.id)
        {
            damage.base_value = f64::from(6 + size);
            damage.dirty.store(true, Ordering::Relaxed);
        }
    }
}
