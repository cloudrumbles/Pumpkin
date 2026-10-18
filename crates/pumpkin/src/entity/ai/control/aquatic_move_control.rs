use super::{
    Control, MoveControlTrait,
    move_control::{MoveControl, Operation},
};
use crate::entity::{
    mob::{Mob, zombie::drowned::DrownedEntity},
    passive::turtle::TurtleEntity,
};
use pumpkin_data::{attributes::Attributes, tracked_data};
use pumpkin_util::math::vector3::Vector3;
use std::sync::atomic::Ordering::Relaxed;

macro_rules! wanted_position {
    () => {
        fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed: f64) {
            self.inner.set_wanted_position(x, y, z, speed);
        }
        fn wanted_position(&self) -> Vector3<f64> {
            self.inner.wanted_position()
        }
        fn speed_modifier(&self) -> f64 {
            self.inner.speed_modifier
        }
        fn has_wanted(&self) -> bool {
            self.inner.has_wanted()
        }
    };
}

fn ease_speed(mob: &dyn Mob, control: &MoveControl) -> f64 {
    let living = &mob.get_mob_entity().living_entity;
    let target =
        (control.speed_modifier * living.get_attribute_value(&Attributes::MOVEMENT_SPEED)) as f32;
    let old = living.ai_speed.load() as f32;
    living.set_ai_speed(f64::from(old + 0.125 * (target - old)));
    living.ai_speed.load()
}

fn face_direction(mob: &dyn Mob, control: &MoveControl, delta: Vector3<f64>) {
    let entity = mob.get_entity();
    let desired = delta.z.atan2(delta.x).to_degrees() as f32 - 90.0;
    let yaw = control
        .change_angle(entity.yaw.load(), desired, 90.0)
        .rem_euclid(360.0);
    entity.yaw.store(yaw);
    entity.body_yaw.store(yaw);
}

#[derive(Default)]
pub struct FishMoveControl {
    inner: MoveControl,
}
impl Control for FishMoveControl {}
impl MoveControlTrait for FishMoveControl {
    wanted_position!();
    fn tick(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        let living = &mob.get_mob_entity().living_entity;
        if entity.is_submerged_in_water() {
            entity
                .velocity
                .store(entity.velocity.load() + Vector3::new(0.0, 0.005, 0.0));
        }
        if self.inner.operation != Operation::MoveTo || mob.is_navigator_idle() {
            living.set_ai_speed(0.0);
            return;
        }
        let speed = ease_speed(mob, &self.inner);
        let delta = self.inner.wanted_position() - entity.pos.load();
        if delta.y != 0.0 {
            entity.velocity.store(
                entity.velocity.load()
                    + Vector3::new(0.0, speed * delta.y / delta.length() * 0.1, 0.0),
            );
        }
        if delta.x != 0.0 || delta.z != 0.0 {
            face_direction(mob, &self.inner, delta);
        }
    }
}

#[derive(Default)]
pub struct GuardianMoveControl {
    inner: MoveControl,
}
impl Control for GuardianMoveControl {}
impl MoveControlTrait for GuardianMoveControl {
    wanted_position!();
    fn tick(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        let living = &mob.get_mob_entity().living_entity;
        let moving = self.inner.operation == Operation::MoveTo && !mob.is_navigator_idle();
        entity.set_synced_data(tracked_data::guardian::DATA_ID_MOVING, moving);
        if !moving {
            living.set_ai_speed(0.0);
            return;
        }
        let delta = self.inner.wanted_position() - entity.pos.load();
        let length = delta.length();
        if length < 1.0e-7 {
            living.set_ai_speed(0.0);
            return;
        }
        let direction = delta / length;
        face_direction(mob, &self.inner, delta);
        let speed = ease_speed(mob, &self.inner);
        let phase = f64::from(entity.tick_count.load(Relaxed) + entity.entity_id);
        let (sin, cos) = f64::from(entity.yaw.load().to_radians()).sin_cos();
        let push = (phase * 0.5).sin() * 0.05;
        let vertical = (phase * 0.75).sin() * 0.05;
        entity.velocity.store(
            entity.velocity.load()
                + Vector3::new(
                    push * cos,
                    vertical * (sin + cos) * 0.25 + speed * direction.y * 0.1,
                    push * sin,
                ),
        );
        let pos = entity.pos.load();
        let wanted = Vector3::new(
            pos.x + direction.x * 2.0,
            entity.get_eye_y() + direction.y / length,
            pos.z + direction.z * 2.0,
        );
        let mut look = mob
            .get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = look.wanted_position().unwrap_or(wanted);
        let next = previous + (wanted - previous) * 0.125;
        look.look_at_with_range(next.x, next.y, next.z, 10.0, 40.0);
    }
}

#[derive(Default)]
pub struct TurtleMoveControl {
    inner: MoveControl,
}
impl Control for TurtleMoveControl {}
impl MoveControlTrait for TurtleMoveControl {
    wanted_position!();
    fn tick(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        let living = &mob.get_mob_entity().living_entity;
        if entity.is_in_water() {
            entity
                .velocity
                .store(entity.velocity.load() + Vector3::new(0.0, 0.005, 0.0));
            if mob
                .cast_any()
                .downcast_ref::<TurtleEntity>()
                .is_some_and(|turtle| {
                    turtle
                        .home_pos
                        .load()
                        .to_centered_f64()
                        .squared_distance_to_vec(&entity.pos.load())
                        >= 256.0
                })
            {
                living.set_ai_speed((living.ai_speed.load() * 0.5).max(0.08));
            }
            if entity.age.load(Relaxed) < 0 {
                living.set_ai_speed((living.ai_speed.load() / 3.0).max(0.06));
            }
        } else if entity.on_ground.load(Relaxed) {
            living.set_ai_speed((living.ai_speed.load() * 0.5).max(0.06));
        }
        if self.inner.operation != Operation::MoveTo || mob.is_navigator_idle() {
            living.set_ai_speed(0.0);
            return;
        }
        let delta = self.inner.wanted_position() - entity.pos.load();
        if delta.length() < 1.0e-5 {
            living.set_ai_speed(0.0);
            return;
        }
        face_direction(mob, &self.inner, delta);
        let speed = ease_speed(mob, &self.inner);
        entity.velocity.store(
            entity.velocity.load() + Vector3::new(0.0, speed * delta.y / delta.length() * 0.1, 0.0),
        );
    }
}

#[derive(Default)]
pub struct DrownedMoveControl {
    inner: MoveControl,
}
impl Control for DrownedMoveControl {}
impl MoveControlTrait for DrownedMoveControl {
    wanted_position!();
    fn tick(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        let living = &mob.get_mob_entity().living_entity;
        let drowned = mob.cast_any().downcast_ref::<DrownedEntity>();
        if entity.is_in_water() && drowned.is_some_and(DrownedEntity::wants_to_swim) {
            if mob
                .get_mob_entity()
                .get_target()
                .is_some_and(|target| target.get_entity().pos.load().y > entity.pos.load().y)
                || drowned.is_some_and(|drowned| drowned.searching_for_land.load(Relaxed))
            {
                entity
                    .velocity
                    .store(entity.velocity.load() + Vector3::new(0.0, 0.002, 0.0));
            }
            if self.inner.operation != Operation::MoveTo || mob.is_navigator_idle() {
                living.set_ai_speed(0.0);
                return;
            }
            let delta = self.inner.wanted_position() - entity.pos.load();
            if delta.length() < 1.0e-7 {
                living.set_ai_speed(0.0);
                return;
            }
            face_direction(mob, &self.inner, delta);
            let speed = ease_speed(mob, &self.inner);
            entity.velocity.store(
                entity.velocity.load()
                    + Vector3::new(
                        speed * delta.x * 0.005,
                        speed * delta.y / delta.length() * 0.1,
                        speed * delta.z * 0.005,
                    ),
            );
        } else {
            if !entity.on_ground.load(Relaxed) {
                entity
                    .velocity
                    .store(entity.velocity.load() + Vector3::new(0.0, -0.008, 0.0));
            }
            self.inner.tick(mob);
        }
    }
}
