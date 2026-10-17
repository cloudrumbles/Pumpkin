use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

use crate::entity::ai::{
    goal::{Controls, Goal},
    pathfinder::{Navigator, NavigatorGoal},
};
use crossbeam::atomic::AtomicCell;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, fluid::Fluid};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use pumpkin_world::world::BlockFlags;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        breed::BreedGoal, escape_danger::EscapeDangerGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

const TEMPT_ITEMS: &[&Item] = &[&Item::SEAGRASS];

pub struct TurtleEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub has_egg: AtomicBool,
    pub laying_egg: AtomicBool,
    pub home_pos: AtomicCell<BlockPos>,
}

impl TurtleEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let home = entity.block_pos.load();
        let mob_entity = MobEntity::new(entity);
        *mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Navigator::amphibious(false);
        let turtle = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            has_egg: AtomicBool::new(false),
            laying_egg: AtomicBool::new(false),
            home_pos: AtomicCell::new(home),
        };
        let mob_arc = Arc::new(turtle);
        let mob_weak: Weak<dyn Mob> = {
            let mob_arc: Arc<dyn Mob> = mob_arc.clone();
            Arc::downgrade(&mob_arc)
        };

        {
            let mut goal_selector = mob_arc
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            goal_selector.add_goal(0, EscapeDangerGoal::new(1.2));
            goal_selector.add_goal(1, BreedGoal::new(1.0));
            goal_selector.add_goal(1, Box::new(TurtleLayEggGoal::default()));
            goal_selector.add_goal(2, Box::new(TemptGoal::new(1.1, TEMPT_ITEMS, false)));
            goal_selector.add_goal(3, Box::new(TurtleJourneyGoal::default()));
            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }

    #[must_use]
    pub fn has_egg(&self) -> bool {
        self.has_egg.load(Ordering::Relaxed)
    }

    pub fn set_has_egg(&self, has_egg: bool) {
        self.has_egg.store(has_egg, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::turtle::HAS_EGG, has_egg);
    }

    #[must_use]
    pub fn is_laying_egg(&self) -> bool {
        self.laying_egg.load(Ordering::Relaxed)
    }

    pub fn set_laying_egg(&self, laying_egg: bool) {
        self.laying_egg.store(laying_egg, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::turtle::LAYING_EGG, laying_egg);
    }
}

impl AgeableMob for TurtleEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for TurtleEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_TURTLE_FOOD)
            || item_stack.item == &Item::SEAGRASS
    }
}

impl Mob for TurtleEntity {
    fn get_mob_gravity(&self) -> f64 {
        crate::entity::ai::goal::aquatic::aquatic_gravity(&self.mob_entity)
    }
    fn create_breed_offspring(
        &self,
        _world: &Arc<crate::world::World>,
    ) -> Option<Arc<dyn EntityBase>> {
        self.set_has_egg(true);
        None
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        nbt.put_bool("has_egg", self.has_egg());
        let home = self.home_pos.load().0;
        nbt.put(
            "home_pos",
            pumpkin_nbt::tag::NbtTag::IntArray(vec![home.x, home.y, home.z]),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        if let Some(home) = nbt.get_int_array("home_pos")
            && home.len() == 3
        {
            self.home_pos
                .store(BlockPos::new(home[0], home[1], home[2]));
        } else if let (Some(x), Some(y), Some(z)) = (
            nbt.get_int("HomePosX"),
            nbt.get_int("HomePosY"),
            nbt.get_int("HomePosZ"),
        ) {
            self.home_pos.store(BlockPos::new(x, y, z));
        }
        if let Some(has_egg) = nbt.get_bool("has_egg").or_else(|| nbt.get_bool("HasEgg")) {
            self.set_has_egg(has_egg);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let growing = self.get_age() == -1 && !self.is_age_locked();
        self.ageable_ai_step();
        if growing
            && self
                .get_entity()
                .world
                .load()
                .level_info
                .load()
                .game_rules
                .mob_drops
        {
            self.get_entity().world.load().drop_stack(
                &self.get_entity().block_pos.load(),
                ItemStack::new(1, &Item::TURTLE_SCUTE),
            );
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::turtle::DATA_BABY_ID, true);
        }
        entity.set_synced_data(pumpkin_data::tracked_data::turtle::HAS_EGG, self.has_egg());
        entity.set_synced_data(
            pumpkin_data::tracked_data::turtle::LAYING_EGG,
            self.is_laying_egg(),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if self.has_egg() && !self.is_baby() {
            return false;
        }
        self.animal_interact(player, item_stack, Sound::EntityTurtleAmbientLand)
    }
}

fn turtle(mob: &dyn Mob) -> Option<&TurtleEntity> {
    mob.cast_any().downcast_ref::<TurtleEntity>()
}
fn center(pos: BlockPos) -> Vector3<f64> {
    pos.to_f64() + Vector3::new(0.5, 0.0, 0.5)
}
fn navigate(mob: &dyn Mob, dest: Vector3<f64>, speed: f64) {
    mob.get_mob_entity()
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .set_progress(NavigatorGoal::new(mob.get_entity().pos.load(), dest, speed));
}
#[derive(Default)]
struct TurtleLayEggGoal {
    target: Option<BlockPos>,
    ticks: i32,
    digging: i32,
    retry: i64,
}
impl Goal for TurtleLayEggGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(t) = turtle(mob) else {
            return false;
        };
        let entity = mob.get_entity();
        let world = entity.world.load();
        if !t.has_egg()
            || entity
                .pos
                .load()
                .squared_distance_to_vec(&center(t.home_pos.load()))
                > 81.0
            || world.get_world_age() < self.retry
        {
            return false;
        }
        self.retry = world.get_world_age() + 40;
        let origin = entity.block_pos.load();
        let mut candidates = Vec::new();
        for x in -16..=16 {
            for z in -16..=16 {
                for y in -3..=3 {
                    let pos = origin.offset(Vector3::new(x, y, z));
                    if world.get_block(&pos).is_air()
                        && world
                            .get_block(&pos.down())
                            .has_tag(&tag::Block::MINECRAFT_SAND)
                    {
                        candidates.push(pos);
                    }
                }
            }
        }
        candidates.sort_by(|a, b| {
            center(*a)
                .squared_distance_to_vec(&entity.pos.load())
                .total_cmp(&center(*b).squared_distance_to_vec(&entity.pos.load()))
        });
        self.target = candidates.into_iter().find(|pos| {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .can_reach_within(&mob.get_mob_entity().living_entity, center(*pos), 1.5)
        });
        self.target.is_some()
    }
    fn start(&mut self, _mob: &dyn Mob) {
        self.ticks = 0;
        self.digging = 0;
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.ticks < 1200 && self.target.is_some() && turtle(mob).is_some_and(TurtleEntity::has_egg)
    }
    fn tick(&mut self, mob: &dyn Mob) {
        self.ticks += 1;
        let (Some(pos), Some(t)) = (self.target, turtle(mob)) else {
            return;
        };
        let entity = mob.get_entity();
        let world = entity.world.load();
        if !world.get_block(&pos).is_air()
            || !world
                .get_block(&pos.down())
                .has_tag(&tag::Block::MINECRAFT_SAND)
        {
            self.target = None;
            return;
        }
        if entity.pos.load().squared_distance_to_vec(&center(pos)) < 2.25
            && !entity.touching_water.load(Ordering::Relaxed)
        {
            if self.digging == 0 {
                t.set_laying_egg(true);
                mob.get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stop();
            }
            self.digging += 1;
            if self.digging > 200 {
                let eggs = rand::random_range(1..=4).to_string();
                let state = Block::TURTLE_EGG
                    .from_properties(&[("eggs", eggs.as_str())])
                    .to_state_id(&Block::TURTLE_EGG);
                world.set_block_state(&pos, state, BlockFlags::NOTIFY_ALL);
                entity.play_sound(Sound::EntityTurtleLayEgg);
                t.set_has_egg(false);
                t.set_laying_egg(false);
                t.mob_entity.love_ticks.store(600, Ordering::Relaxed);
                self.target = None;
            }
        } else if self.ticks % 20 == 1 {
            navigate(mob, center(pos), 1.0);
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(t) = turtle(mob) {
            t.set_laying_egg(false);
        }
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
struct TurtleJourneyGoal {
    destination: Option<Vector3<f64>>,
    ticks: i32,
    home: bool,
    retry: i64,
}
impl Goal for TurtleJourneyGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(t) = turtle(mob) else {
            return false;
        };
        let entity = mob.get_entity();
        let world = entity.world.load();
        if world.get_world_age() < self.retry {
            return false;
        }
        self.retry = world.get_world_age() + 40;
        self.home = !t.is_baby()
            && (t.has_egg()
                || (rand::random_range(0..18) == 0
                    && entity
                        .pos
                        .load()
                        .squared_distance_to_vec(&center(t.home_pos.load()))
                        > 4096.0));
        if self.home {
            self.destination = Some(center(t.home_pos.load()));
            return entity
                .pos
                .load()
                .squared_distance_to_vec(&center(t.home_pos.load()))
                > 49.0;
        }
        if !entity.touching_water.load(Ordering::Relaxed) && !t.has_egg() {
            let mut water = Vec::new();
            let origin = entity.block_pos.load();
            for x in -24..=24 {
                for z in -24..=24 {
                    for y in -4..=1 {
                        let pos = origin.offset(Vector3::new(x, y, z));
                        if world.get_fluid(&pos).matches_type(&Fluid::WATER) {
                            water.push(center(pos));
                        }
                    }
                }
            }
            water.sort_by(|a, b| {
                a.squared_distance_to_vec(&entity.pos.load())
                    .total_cmp(&b.squared_distance_to_vec(&entity.pos.load()))
            });
            self.destination = water.into_iter().find(|pos| {
                mob.get_mob_entity()
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .can_reach_within(&mob.get_mob_entity().living_entity, *pos, 1.0)
            });
            return self.destination.is_some();
        }
        if entity.touching_water.load(Ordering::Relaxed)
            && !t.has_egg()
            && !t.mob_entity.is_in_love()
        {
            for _ in 0..10 {
                let pos = entity.pos.load()
                    + Vector3::new(
                        rand::random_range(-16.0..=16.0),
                        rand::random_range(-3.0..=3.0),
                        rand::random_range(-16.0..=16.0),
                    );
                if world
                    .get_fluid(&BlockPos::floored_v(pos))
                    .matches_type(&Fluid::WATER)
                {
                    self.destination = Some(pos);
                    return true;
                }
            }
        }
        false
    }
    fn start(&mut self, _mob: &dyn Mob) {
        self.ticks = 0;
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.ticks < 1200
            && self.destination.is_some_and(|p| {
                p.squared_distance_to_vec(&mob.get_entity().pos.load())
                    > if self.home { 49.0 } else { 1.0 }
            })
    }
    fn tick(&mut self, mob: &dyn Mob) {
        self.ticks += 1;
        if self.ticks % 40 != 1 && !mob.is_navigator_idle() {
            return;
        }
        if let Some(dest) = self.destination {
            let pos = mob.get_entity().pos.load();
            let delta = dest - pos;
            let next = if delta.length() > 16.0 {
                pos + delta.normalize() * 16.0
            } else {
                dest
            };
            navigate(
                mob,
                next,
                if turtle(mob).is_some_and(AgeableMob::is_baby) {
                    2.0
                } else {
                    1.0
                },
            );
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        self.destination = None;
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
