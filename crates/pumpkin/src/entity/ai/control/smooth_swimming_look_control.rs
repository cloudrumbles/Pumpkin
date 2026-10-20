use super::{Control, look_control::LookControl};
use crate::entity::mob::Mob;
use pumpkin_util::math::wrap_degrees;

#[derive(Clone, Copy)]
pub struct SmoothSwimmingLookControl {
    max_y_rot_from_center: i32,
}

impl SmoothSwimmingLookControl {
    #[must_use]
    pub const fn new(max_y_rot_from_center: i32) -> Self {
        Self {
            max_y_rot_from_center,
        }
    }

    pub fn tick(self, look: &mut LookControl, mob: &dyn Mob) {
        let entity = mob.get_entity();
        if look.look_at_timer > 0 {
            look.look_at_timer -= 1;
            if let Some(yaw) = look.get_target_yaw(mob.get_mob_entity()) {
                entity.head_yaw.store(look.change_angle(
                    entity.head_yaw.load(),
                    yaw + 20.0,
                    look.max_yaw_change,
                ));
            }
            if let Some(pitch) = look.get_target_pitch(mob.get_mob_entity()) {
                entity.set_pitch(look.change_angle(
                    entity.pitch.load(),
                    pitch + 10.0,
                    look.max_pitch_change,
                ));
            }
        } else {
            if mob.is_navigator_idle() {
                entity.set_pitch(look.change_angle(entity.pitch.load(), 0.0, 5.0));
            }
            entity.head_yaw.store(look.change_angle(
                entity.head_yaw.load(),
                entity.body_yaw.load(),
                look.max_yaw_change,
            ));
        }
        let difference = wrap_degrees(entity.head_yaw.load() - entity.body_yaw.load());
        let limit = self.max_y_rot_from_center as f32;
        if difference < -limit {
            entity.body_yaw.store(entity.body_yaw.load() - 4.0);
        } else if difference > limit {
            entity.body_yaw.store(entity.body_yaw.load() + 4.0);
        }
    }
}
