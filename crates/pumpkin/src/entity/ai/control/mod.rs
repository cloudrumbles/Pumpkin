use crate::entity::mob::Mob;
use pumpkin_util::math::subtract_angles;

pub mod aquatic_move_control;
pub mod body_rotation_control;
pub mod flying_move_control;
pub mod jump_control;
pub mod look_control;
pub mod move_control;
pub mod smooth_swimming_look_control;
pub mod smooth_swimming_move_control;

pub trait Control: Send + Sync {
    fn change_angle(&self, start: f32, end: f32, max_change: f32) -> f32 {
        let i = subtract_angles(start, end);
        let j = i.clamp(-max_change, max_change);
        start + j
    }
}

pub trait MoveControlTrait: Control {
    fn tick(&mut self, mob: &dyn Mob);

    fn set_wanted_position(&mut self, _x: f64, _y: f64, _z: f64, _speed_modifier: f64) {}

    fn strafe(&mut self, _forward: f32, _right: f32) {}

    fn wanted_position(&self) -> pumpkin_util::math::vector3::Vector3<f64> {
        pumpkin_util::math::vector3::Vector3::default()
    }
    fn speed_modifier(&self) -> f64 {
        0.0
    }
    fn has_wanted(&self) -> bool {
        false
    }
}

/// Select the species' controller independently of its current navigation path.
pub fn movement_controller(kind: &pumpkin_data::entity::EntityType) -> Box<dyn MoveControlTrait> {
    use aquatic_move_control::{
        DrownedMoveControl, FishMoveControl, GuardianMoveControl, TurtleMoveControl,
    };
    use flying_move_control::FlyingMoveControl;
    use pumpkin_data::entity::EntityType as E;
    use smooth_swimming_move_control::SmoothSwimmingMoveControl;
    if [&E::COD, &E::SALMON, &E::TROPICAL_FISH, &E::PUFFERFISH].contains(&kind) {
        Box::new(FishMoveControl::default())
    } else if [&E::GUARDIAN, &E::ELDER_GUARDIAN].contains(&kind) {
        Box::new(GuardianMoveControl::default())
    } else if kind == &E::TURTLE {
        Box::new(TurtleMoveControl::default())
    } else if kind == &E::DROWNED {
        Box::new(DrownedMoveControl::default())
    } else if [&E::BEE, &E::ALLAY].contains(&kind) {
        Box::new(FlyingMoveControl::new(20, true))
    } else if kind == &E::PARROT {
        Box::new(FlyingMoveControl::new(10, false))
    } else if kind == &E::HAPPY_GHAST {
        Box::new(FlyingMoveControl::new(180, true))
    } else if kind == &E::AXOLOTL {
        Box::new(SmoothSwimmingMoveControl::new(85, 10, 0.1, 0.5, false))
    } else if [&E::NAUTILUS, &E::ZOMBIE_NAUTILUS].contains(&kind) {
        Box::new(SmoothSwimmingMoveControl::new(85, 10, 0.011, 0.0, true))
    } else if [&E::DOLPHIN, &E::FROG, &E::TADPOLE].contains(&kind) {
        Box::new(SmoothSwimmingMoveControl::new(85, 10, 0.02, 0.1, true))
    } else if kind == &E::SULFUR_CUBE {
        Box::new(DirectMoveControl)
    } else {
        Box::new(move_control::MoveControl::default())
    }
}
struct DirectMoveControl;
impl Control for DirectMoveControl {}
impl MoveControlTrait for DirectMoveControl {
    fn tick(&mut self, _mob: &dyn Mob) {}
}
