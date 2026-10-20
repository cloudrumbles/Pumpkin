use crate::entity::ai::control::{Control, MoveControlTrait};
use crate::entity::ai::pathfinder::{node::PathType, pathfinding_context::PathfindingContext};
use crate::entity::mob::Mob;
use pumpkin_data::{
    attributes::Attributes,
    tag::{self, Taggable},
};
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use std::sync::atomic::Ordering;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    #[default]
    Wait,
    MoveTo,
    Strafe,
    Jumping,
}

pub struct MoveControl {
    pub wanted_x: f64,
    pub wanted_y: f64,
    pub wanted_z: f64,
    pub speed_modifier: f64,
    pub strafe_forwards: f32,
    pub strafe_right: f32,
    pub operation: Operation,
}

impl Default for MoveControl {
    fn default() -> Self {
        Self {
            wanted_x: 0.0,
            wanted_y: 0.0,
            wanted_z: 0.0,
            speed_modifier: 0.0,
            strafe_forwards: 0.0,
            strafe_right: 0.0,
            operation: Operation::Wait,
        }
    }
}

impl Control for MoveControl {}

impl MoveControlTrait for MoveControl {
    fn tick(&mut self, mob: &dyn Mob) {
        let mob_entity = mob.get_mob_entity();
        let living_entity = &mob_entity.living_entity;
        let entity = &living_entity.entity;
        if self.operation == Operation::Strafe {
            let speed = self.speed_modifier
                * living_entity.get_attribute_value(&Attributes::MOVEMENT_SPEED);
            let scale = speed / f64::from(self.strafe_forwards.hypot(self.strafe_right).max(1.0));
            let xa = f64::from(self.strafe_forwards) * scale;
            let za = f64::from(self.strafe_right) * scale;
            let (sin, cos) = f64::from(entity.yaw.load()).to_radians().sin_cos();
            let test =
                entity.pos.load() + Vector3::new(xa * cos - za * sin, 0.0, za * cos + xa * sin);
            let mut context =
                PathfindingContext::new(entity.block_pos.load().0, entity.world.load_full());
            if context.get_land_node_type(BlockPos::floored_v(test).0) != PathType::Walkable {
                self.strafe_forwards = 1.0;
                self.strafe_right = 0.0;
            }
            living_entity.set_ai_speed(speed);
            living_entity.movement_input.store(Vector3::new(
                self.strafe_right as f64,
                0.0,
                self.strafe_forwards as f64,
            ));
            self.operation = Operation::Wait;
        } else if self.operation == Operation::MoveTo {
            self.operation = Operation::Wait;
            let pos = entity.pos.load();
            let xd = self.wanted_x - pos.x;
            let zd = self.wanted_z - pos.z;
            let yd = self.wanted_y - pos.y;
            let dd = xd * xd + yd * yd + zd * zd;

            if dd < 2.5000003E-7 {
                living_entity
                    .movement_input
                    .store(Vector3::new(0.0, 0.0, 0.0));
                return;
            }

            let y_rot_d = (zd.atan2(xd).to_degrees() as f32) - 90.0;
            entity
                .yaw
                .store(self.change_angle(entity.yaw.load(), y_rot_d, 90.0));

            let movement_speed = living_entity.get_attribute_value(&Attributes::MOVEMENT_SPEED);
            let speed = self.speed_modifier * movement_speed;
            living_entity.set_ai_speed(speed);

            let step_height = living_entity.get_attribute_value(&Attributes::STEP_HEIGHT);
            let block_pos = entity.block_pos.load();
            let world = entity.world.load();
            let (block, state) = world.get_block_and_state(&block_pos);
            let embedded = state
                .get_block_collision_shapes_at(&block_pos)
                .any(|shape| entity.pos.load().y < shape.max.y + f64::from(block_pos.0.y))
                && !block.has_tag(&tag::Block::MINECRAFT_DOORS)
                && !block.has_tag(&tag::Block::MINECRAFT_FENCES);
            if (yd > step_height
                && xd * xd + zd * zd < 1.0f64.max(f64::from(entity.entity_dimension.load().width)))
                || embedded
            {
                living_entity.jumping.store(true, Ordering::SeqCst);
                self.operation = Operation::Jumping;
            }
        } else if self.operation == Operation::Jumping {
            let movement_speed = living_entity.get_attribute_value(&Attributes::MOVEMENT_SPEED);
            let speed = self.speed_modifier * movement_speed;
            living_entity.set_ai_speed(speed);

            if entity.on_ground.load(Ordering::Relaxed)
                || entity.touching_water.load(Ordering::Relaxed)
                || entity.touching_lava.load(Ordering::Relaxed)
            {
                self.operation = Operation::Wait;
            }
        } else {
            let mut input = living_entity.movement_input.load();
            input.z = 0.0;
            living_entity.movement_input.store(input);
        }
    }

    fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.wanted_x = x;
        self.wanted_y = y;
        self.wanted_z = z;
        self.speed_modifier = speed_modifier;
        if self.operation != Operation::Jumping {
            self.operation = Operation::MoveTo;
        }
    }

    fn strafe(&mut self, forwards: f32, right: f32) {
        self.operation = Operation::Strafe;
        self.strafe_forwards = forwards;
        self.strafe_right = right;
        self.speed_modifier = 0.25;
    }

    fn wanted_position(&self) -> Vector3<f64> {
        Vector3::new(self.wanted_x, self.wanted_y, self.wanted_z)
    }
    fn speed_modifier(&self) -> f64 {
        self.speed_modifier
    }
    fn has_wanted(&self) -> bool {
        self.operation == Operation::MoveTo
    }
}

impl MoveControl {
    #[must_use]
    pub fn has_wanted(&self) -> bool {
        self.operation == Operation::MoveTo
    }

    #[must_use]
    pub const fn get_speed_modifier(&self) -> f64 {
        self.speed_modifier
    }

    pub fn set_wanted_position(&mut self, x: f64, y: f64, z: f64, speed_modifier: f64) {
        self.wanted_x = x;
        self.wanted_y = y;
        self.wanted_z = z;
        self.speed_modifier = speed_modifier;
        if self.operation != Operation::Jumping {
            self.operation = Operation::MoveTo;
        }
    }

    pub const fn strafe(&mut self, forwards: f32, right: f32) {
        self.operation = Operation::Strafe;
        self.strafe_forwards = forwards;
        self.strafe_right = right;
        self.speed_modifier = 0.25;
    }
}
