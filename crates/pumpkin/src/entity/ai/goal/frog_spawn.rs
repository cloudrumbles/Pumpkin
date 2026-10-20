use super::{Controls, Goal};
use crate::entity::{
    ai::{brain::memory::types, pathfinder::NavigatorGoal},
    mob::Mob,
};
use pumpkin_data::{Block, entity::EntityPose, fluid::Fluid, sound::Sound};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use pumpkin_world::world::BlockFlags;
use std::sync::atomic::Ordering;

#[derive(Default)]
pub struct FrogSpawnGoal {
    target: Option<(BlockPos, Vector3<f64>)>,
    timer: i32,
    next_search: i64,
}

impl Goal for FrogSpawnGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if !mob
            .get_mob_entity()
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .has_memory_value(types::IS_PREGNANT.id())
        {
            return false;
        }
        let entity = mob.get_entity();
        let world = entity.world.load();
        if world.get_world_age() < self.next_search {
            return false;
        }
        self.next_search = world.get_world_age() + 40;
        let origin = entity.block_pos.load();
        let mut candidates = Vec::new();
        for x in -8..=8 {
            for z in -8..=8 {
                for y in -3..=3 {
                    let spawn = origin.offset(Vector3::new(x, y, z));
                    if !world.get_block(&spawn).is_air()
                        || !world.get_fluid(&spawn.down()).matches_type(&Fluid::WATER)
                    {
                        continue;
                    }
                    for step in [
                        Vector3::new(1, 0, 0),
                        Vector3::new(-1, 0, 0),
                        Vector3::new(0, 0, 1),
                        Vector3::new(0, 0, -1),
                    ] {
                        let stand = spawn.offset(step);
                        if world.get_block(&stand.down()).is_solid()
                            && world.get_block(&stand).is_air()
                        {
                            let pos = Vector3::new(
                                f64::from(stand.0.x) + 0.5,
                                f64::from(stand.0.y),
                                f64::from(stand.0.z) + 0.5,
                            );
                            candidates.push((spawn, pos));
                        }
                    }
                }
            }
        }
        candidates.sort_by(|a, b| {
            entity
                .pos
                .load()
                .squared_distance_to_vec(&a.1)
                .total_cmp(&entity.pos.load().squared_distance_to_vec(&b.1))
        });
        self.target = candidates.into_iter().find(|(_, pos)| {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .can_reach_within(&mob.get_mob_entity().living_entity, *pos, 1.0)
        });
        self.target.is_some()
    }
    fn start(&mut self, _mob: &dyn Mob) {
        self.timer = 0;
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.target.is_some()
            && self.timer < 200
            && mob
                .get_mob_entity()
                .brain
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .has_memory_value(types::IS_PREGNANT.id())
    }
    fn tick(&mut self, mob: &dyn Mob) {
        self.timer += 1;
        let Some((spawn, stand)) = self.target else {
            return;
        };
        let entity = mob.get_entity();
        let world = entity.world.load();
        if entity.pos.load().squared_distance_to_vec(&stand) < 1.25
            && entity.on_ground.load(Ordering::Relaxed)
            && world.get_block(&spawn).is_air()
            && world.get_fluid(&spawn.down()).matches_type(&Fluid::WATER)
        {
            world.set_block_state(
                &spawn,
                Block::FROGSPAWN.default_state.id,
                BlockFlags::NOTIFY_ALL,
            );
            entity.play_sound(Sound::EntityFrogLaySpawn);
            mob.get_mob_entity()
                .brain
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .erase(types::IS_PREGNANT.id());
            self.target = None;
        } else if self.timer % 10 == 1 {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_progress(NavigatorGoal::new(entity.pos.load(), stand, 1.0));
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        self.target = None;
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

#[derive(Default)]
pub struct FrogCroakGoal {
    ticks: i32,
}
impl Goal for FrogCroakGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_entity().on_ground.load(Ordering::Relaxed)
            && !mob.get_entity().touching_water.load(Ordering::Relaxed)
            && mob.is_navigator_idle()
            && rand::random_range(0..100) == 0
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.ticks = 60;
        mob.get_entity().set_pose(EntityPose::Croaking);
        mob.get_entity().play_sound(Sound::EntityFrogAmbient);
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.ticks > 0 && !mob.get_entity().touching_water.load(Ordering::Relaxed)
    }
    fn tick(&mut self, _mob: &dyn Mob) {
        self.ticks -= 1;
    }
    fn stop(&mut self, mob: &dyn Mob) {
        mob.get_entity().set_pose(EntityPose::Standing);
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
