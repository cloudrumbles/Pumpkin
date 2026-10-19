use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering::Relaxed},
};

use pumpkin_data::{
    Block,
    tag::{self, Taggable},
    world::WorldEvent,
};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

use crate::{
    entity::{mob::Mob, passive::rabbit::RabbitEntity},
    world::World,
};

use super::{
    Controls, Goal, ParentHandle,
    move_to_target_pos::{MoveToTargetPos, MoveToTargetPosGoal},
};

/// Carrot raiding uses the same search, approach and retry schedule as MoveToBlockGoal.
pub struct RabbitRaidGoal {
    approach: MoveToTargetPosGoal<Self>,
    wants_to_raid: AtomicBool,
    can_raid: AtomicBool,
}

impl RabbitRaidGoal {
    #[must_use]
    pub fn new() -> Box<Self> {
        let mut goal = Box::new(Self {
            approach: MoveToTargetPosGoal::with_default(
                ParentHandle::none(),
                f64::from(0.7f32),
                16,
            ),
            wants_to_raid: AtomicBool::new(false),
            can_raid: AtomicBool::new(false),
        });
        // SAFETY: The boxed goal stays at this address and owns the child handle.
        goal.approach.move_to_target_pos = unsafe { ParentHandle::new(goal.as_ref()) };
        goal
    }
}

impl MoveToTargetPos for RabbitRaidGoal {
    fn is_target_pos(&self, world: Arc<World>, pos: BlockPos) -> bool {
        if !self.wants_to_raid.load(Relaxed)
            || self.can_raid.load(Relaxed)
            || !world
                .get_block(&pos)
                .has_tag(&tag::Block::MINECRAFT_SUPPORTS_CROPS)
        {
            return false;
        }
        let (block, state) = world.get_block_and_state_id(&pos.up());
        let mature = block == &Block::CARROTS
            && block.properties(state).is_some_and(|properties| {
                properties
                    .to_props()
                    .iter()
                    .any(|(key, value)| *key == "age" && *value == "7")
            });
        if mature {
            self.can_raid.store(true, Relaxed);
        }
        mature
    }
}

impl Goal for RabbitRaidGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(rabbit) = mob.cast_any().downcast_ref::<RabbitEntity>() else {
            return false;
        };
        if self.approach.cooldown <= 0 {
            if !mob
                .get_entity()
                .world
                .load()
                .level_info
                .load()
                .game_rules
                .mob_griefing
            {
                return false;
            }
            self.can_raid.store(false, Relaxed);
            self.wants_to_raid
                .store(rabbit.more_carrot_ticks.load(Relaxed) <= 0, Relaxed);
        }
        self.approach.can_start(mob)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_raid.load(Relaxed) && self.approach.should_continue(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.approach.start(mob);
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.approach.tick(mob);
        let crop_pos = self.approach.target_pos.up();
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at(
                mob,
                f64::from(crop_pos.0.x) + 0.5,
                f64::from(crop_pos.0.y) + 1.0,
                f64::from(crop_pos.0.z) + 0.5,
            );
        if !self.approach.is_reached_target() {
            return;
        }
        let entity = mob.get_entity();
        let world = entity.world.load_full();
        let (block, state) = world.get_block_and_state_id(&crop_pos);
        if self.can_raid.load(Relaxed)
            && block == &Block::CARROTS
            && let Some(properties) = block.properties(state)
        {
            if let Some(age) = properties
                .to_props()
                .iter()
                .find(|(key, _)| *key == "age")
                .and_then(|(_, value)| value.parse::<i32>().ok())
            {
                if age == 0 {
                    world.break_block(&crop_pos, None, BlockFlags::NOTIFY_ALL);
                } else {
                    let next = (age - 1).to_string();
                    // Carrots have one property: age. A locally borrowed
                    // string avoids leaking a temporary into the block state.
                    let fields = [("age", next.as_str())];
                    world.set_block_state(
                        &crop_pos,
                        block.from_properties(&fields).to_state_id(block),
                        BlockFlags::NOTIFY_LISTENERS,
                    );
                    world.emit_game_event_with_source(
                        "minecraft:block_change",
                        crop_pos.to_centered_f64(),
                        Some(entity),
                    );
                    world.sync_world_event(
                        WorldEvent::ParticlesAndSoundDestroyBlock,
                        crop_pos,
                        i32::from(state.as_u16()),
                    );
                }
                if let Some(rabbit) = mob.cast_any().downcast_ref::<RabbitEntity>() {
                    rabbit.more_carrot_ticks.store(40, Relaxed);
                }
            }
        }
        self.can_raid.store(false, Relaxed);
        self.approach.cooldown = 10;
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::JUMP
    }
}
