use std::sync::atomic::Ordering::Relaxed;

use pumpkin_util::math::vector3::Vector3;

use crate::entity::{mob::Mob, passive::rabbit::RabbitEntity};

use super::{
    Control, MoveControlTrait,
    move_control::{MoveControl, Operation},
};

/// Rabbit movement is a sequence of hops with a pause after each landing.
/// Keep the requested path speed while the ground controller temporarily stops.
#[derive(Default)]
pub struct RabbitMoveControl {
    inner: MoveControl,
    next_jump_speed: f64,
    in_water: bool,
}

impl Control for RabbitMoveControl {}

impl MoveControlTrait for RabbitMoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(rabbit) = mob.cast_any().downcast_ref::<RabbitEntity>() else {
            return;
        };
        let living = &mob.get_mob_entity().living_entity;
        self.in_water = mob.get_entity().is_in_water();
        let speed = if mob.get_entity().on_ground.load(Relaxed)
            && !living.jumping.load(Relaxed)
            && !rabbit.jump_requested.load(Relaxed)
        {
            Some(0.0)
        } else if self.inner.has_wanted() || self.inner.operation == Operation::Jumping {
            Some(self.next_jump_speed)
        } else {
            None
        };
        if let Some(speed) = speed {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_speed(speed);
            self.set_wanted_position(
                self.inner.wanted_x,
                self.inner.wanted_y,
                self.inner.wanted_z,
                speed,
            );
        }
        self.inner.tick(mob);
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed: f64) {
        let speed = if self.in_water { 1.5 } else { speed };
        self.inner.set_wanted_position(x, y, z, speed);
        if speed > 0.0 {
            self.next_jump_speed = speed;
        }
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
}
