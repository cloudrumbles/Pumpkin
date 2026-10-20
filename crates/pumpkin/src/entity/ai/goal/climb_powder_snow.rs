use std::sync::atomic::Ordering::Relaxed;

use pumpkin_data::{
    Block,
    tag::{self, Taggable},
};

use crate::entity::mob::Mob;

use super::{Controls, Goal};

#[derive(Default)]
pub struct ClimbPowderSnowGoal;

impl Goal for ClimbPowderSnowGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        if !(entity.is_in_powder_snow.load(Relaxed) || entity.was_in_powder_snow.load(Relaxed))
            || !entity
                .entity_type
                .has_tag(&tag::EntityType::MINECRAFT_POWDER_SNOW_WALKABLE_MOBS)
        {
            return false;
        }
        let above = entity.block_pos.load().up();
        let world = entity.world.load();
        let (block, state) = world.get_block_and_state(&above);
        block == &Block::POWDER_SNOW || state.get_block_collision_shapes_at(&above).next().is_none()
    }
    fn tick(&mut self, mob: &dyn Mob) {
        mob.request_jump();
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::JUMP
    }
}
