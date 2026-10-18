use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, AtomicU8, Ordering},
};

use crate::block::entities::beehive::BeehiveBlockEntity;
use crate::entity::ai::{
    goal::{Controls, Goal, water_avoiding_random_flying::WaterAvoidingRandomFlyingGoal},
    pathfinder::{Navigator, NavigatorGoal},
};
use crossbeam::atomic::AtomicCell;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{
    Block, attributes::Attributes, damage::DamageType, effect::StatusEffect, particle::Particle,
    potion::Effect,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use pumpkin_world::world::BlockFlags;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        breed::BreedGoal, follow_parent::FollowParentGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, revenge::RevengeGoal, swim::SwimGoal, tempt::TemptGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

pub const FLAG_ROLL: u8 = 2;
pub const FLAG_HAS_STUNG: u8 = 4;
pub const FLAG_HAS_NECTAR: u8 = 8;

pub struct BeeEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub flags: AtomicU8,
    pub ticks_without_nectar: AtomicI32,
    pub cannot_enter_hive_ticks: AtomicI32,
    pub crops_grown_since_pollination: AtomicI32,
    pub time_since_sting: AtomicI32,
    pub hive_position: AtomicCell<Option<BlockPos>>,
    pub flower_position: AtomicCell<Option<BlockPos>>,
    underwater_ticks: AtomicI32,
}

impl BeeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        *mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Navigator::flying();
        let bee = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            flags: AtomicU8::new(0),
            ticks_without_nectar: AtomicI32::new(0),
            cannot_enter_hive_ticks: AtomicI32::new(0),
            crops_grown_since_pollination: AtomicI32::new(0),
            time_since_sting: AtomicI32::new(0),
            hive_position: AtomicCell::new(None),
            flower_position: AtomicCell::new(None),
            underwater_ticks: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(bee);
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

            goal_selector.add_goal(0, Box::new(BeeStingGoal));
            goal_selector.add_goal(1, Box::new(BeeHiveGoal::default()));
            goal_selector.add_goal(4, Box::new(BeePollinateGoal::default()));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(
                3,
                Box::new(TemptGoal::with_tag(
                    1.25,
                    &tag::Item::MINECRAFT_BEE_FOOD,
                    2.5,
                )),
            );
            goal_selector.add_goal(5, Box::new(FollowParentGoal::new(1.25)));
            goal_selector.add_goal(8, Box::new(WaterAvoidingRandomFlyingGoal::new(1.0)));
            goal_selector.add_goal(9, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                10,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(11, Box::new(RandomLookAroundGoal::default()));
        };

        {
            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            target_selector.add_goal(
                1,
                Box::new(
                    RevengeGoal::new(true)
                        .alerting_others_when(|mob, _, attacker| {
                            mob.has_line_of_sight(attacker.get_entity())
                        })
                        .continuing_when(|mob| mob.can_target_while_angry()),
                ),
            );
        };

        mob_arc
    }

    fn wants_hive(&self) -> bool {
        let entity = self.get_entity();
        !self.has_stung()
            && self.cannot_enter_hive_ticks.load(Ordering::Relaxed) <= 0
            && !self.mob_entity.anger.is_angry(self)
            && (self.has_nectar()
                || self.ticks_without_nectar.load(Ordering::Relaxed) > 3600
                || entity
                    .world
                    .load()
                    .bees_stay_in_hive(&entity.block_pos.load()))
    }
    pub fn become_angry_at(&self, target: &dyn EntityBase) {
        if self.has_stung() || !crate::entity::ai::goal::is_valid_attack_target(self, target) {
            return;
        }
        let entity = self.get_entity();
        let world = entity.world.load();
        if let Some(target) = world.get_entity_by_uuid(target.get_entity().entity_uuid) {
            self.mob_entity
                .anger
                .target
                .store(Some(target.get_entity().entity_uuid));
            self.mob_entity.anger.refresh(self);
            self.mob_entity.set_target(Some(target));
        }
    }
    fn grow_crop(&self) {
        if !self.has_nectar()
            || self.hive_position.load().is_none()
            || self.crops_grown_since_pollination.load(Ordering::Relaxed) >= 10
            || rand::random_range(0..30) != 0
        {
            return;
        }
        let entity = self.get_entity();
        let world = entity.world.load();
        for down in 1..=2 {
            let pos = entity.block_pos.load().offset(Vector3::new(0, -down, 0));
            let (block, state) = world.get_block_and_state_id(&pos);
            if !block.has_tag(&tag::Block::MINECRAFT_BEE_GROWABLES) {
                continue;
            }
            let Some(properties) = block.properties(state) else {
                continue;
            };
            let mut props = properties.to_props();
            if let Some((_, value)) = props.iter_mut().find(|(key, _)| *key == "age") {
                let max = if block == &Block::BEETROOTS || block == &Block::SWEET_BERRY_BUSH {
                    3
                } else if block == &Block::TORCHFLOWER_CROP {
                    2
                } else {
                    7
                };
                let age = value.parse::<i32>().unwrap_or(max);
                if age >= max {
                    continue;
                }
                let next = (age + 1).to_string();
                *value = &next;
                world.set_block_state(
                    &pos,
                    block.from_properties(&props).to_state_id(block),
                    BlockFlags::NOTIFY_ALL,
                );
            } else if let Some((_, value)) = props.iter_mut().find(|(key, _)| *key == "berries") {
                if *value == "true" {
                    continue;
                }
                *value = "true";
                world.set_block_state(
                    &pos,
                    block.from_properties(&props).to_state_id(block),
                    BlockFlags::NOTIFY_ALL,
                );
            } else {
                continue;
            }
            self.crops_grown_since_pollination
                .fetch_add(1, Ordering::Relaxed);
            world.spawn_particle(
                pos.to_centered_f64(),
                Vector3::new(0.4, 0.4, 0.4),
                0.0,
                4,
                Particle::HappyVillager,
            );
        }
    }

    #[must_use]
    pub fn has_flag(&self, flag: u8) -> bool {
        (self.flags.load(Ordering::Relaxed) & flag) != 0
    }

    pub fn set_flag(&self, flag: u8, val: bool) {
        let current = self.flags.load(Ordering::Relaxed);
        let new_flags = if val { current | flag } else { current & !flag };
        self.flags.store(new_flags, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::bee::DATA_FLAGS_ID,
            new_flags as i8,
        );
    }

    #[must_use]
    pub fn has_nectar(&self) -> bool {
        self.has_flag(FLAG_HAS_NECTAR)
    }

    pub fn set_has_nectar(&self, val: bool) {
        self.set_flag(FLAG_HAS_NECTAR, val);
    }

    #[must_use]
    pub fn has_stung(&self) -> bool {
        self.has_flag(FLAG_HAS_STUNG)
    }

    pub fn set_has_stung(&self, val: bool) {
        self.set_flag(FLAG_HAS_STUNG, val);
    }

    #[must_use]
    pub fn is_rolling(&self) -> bool {
        self.has_flag(FLAG_ROLL)
    }

    pub fn set_rolling(&self, val: bool) {
        self.set_flag(FLAG_ROLL, val);
    }
}

impl AgeableMob for BeeEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for BeeEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_FLOWERS)
            || item_stack.item.has_tag(&tag::Item::MINECRAFT_BEE_FOOD)
    }
}

impl Mob for BeeEntity {
    fn get_mob_gravity(&self) -> f64 {
        0.0
    }
    fn get_mob_y_velocity_drag(&self) -> Option<f64> {
        Some(0.91)
    }
    fn on_damage(&self, _damage: DamageType, source: Option<&dyn EntityBase>) {
        if let Some(target) = source {
            self.become_angry_at(target);
        }
    }
    fn custom_server_ai_step(&self, caller: &dyn EntityBase) {
        let entity = self.get_entity();
        if self.cannot_enter_hive_ticks.load(Ordering::Relaxed) > 0 {
            self.cannot_enter_hive_ticks.fetch_sub(1, Ordering::Relaxed);
        }
        if !self.has_nectar() {
            self.ticks_without_nectar.fetch_add(1, Ordering::Relaxed);
        }
        if self.has_stung() {
            self.mob_entity.anger.stop(self);
        }
        self.set_rolling(self.mob_entity.get_target().is_some_and(|target| {
            entity
                .pos
                .load()
                .squared_distance_to_vec(&target.get_entity().pos.load())
                < 4.0
        }));
        if entity.touching_water.load(Ordering::Relaxed) {
            if self.underwater_ticks.fetch_add(1, Ordering::Relaxed) > 20 {
                caller.damage(caller, 1.0, DamageType::DROWN);
            }
        } else {
            self.underwater_ticks.store(0, Ordering::Relaxed);
        }
        self.grow_crop();
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        for (key, pos) in [
            ("hive_pos", self.hive_position.load()),
            ("flower_pos", self.flower_position.load()),
        ] {
            if let Some(pos) = pos {
                nbt.put(
                    key,
                    pumpkin_nbt::tag::NbtTag::IntArray(vec![pos.0.x, pos.0.y, pos.0.z]),
                );
            }
        }
        nbt.put_bool("HasNectar", self.has_nectar());
        nbt.put_bool("HasStung", self.has_stung());
        nbt.put_int(
            "TicksSincePollination",
            self.ticks_without_nectar.load(Ordering::Relaxed),
        );
        nbt.put_int(
            "CannotEnterHiveTicks",
            self.cannot_enter_hive_ticks.load(Ordering::Relaxed),
        );
        nbt.put_int(
            "CropsGrownSincePollination",
            self.crops_grown_since_pollination.load(Ordering::Relaxed),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        for (key, field) in [
            ("hive_pos", &self.hive_position),
            ("flower_pos", &self.flower_position),
        ] {
            if let Some(pos) = nbt.get_int_array(key)
                && pos.len() == 3
            {
                field.store(Some(BlockPos::new(pos[0], pos[1], pos[2])));
            }
        }
        if let Some(nectar) = nbt.get_bool("HasNectar") {
            self.set_has_nectar(nectar);
        }
        if let Some(stung) = nbt.get_bool("HasStung") {
            self.set_has_stung(stung);
        }
        if let Some(ticks) = nbt.get_int("TicksSincePollination") {
            self.ticks_without_nectar.store(ticks, Ordering::Relaxed);
        }
        if let Some(cannot) = nbt.get_int("CannotEnterHiveTicks") {
            self.cannot_enter_hive_ticks
                .store(cannot, Ordering::Relaxed);
        }
        if let Some(crops) = nbt.get_int("CropsGrownSincePollination") {
            self.crops_grown_since_pollination
                .store(crops, Ordering::Relaxed);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick(&self, caller: &dyn EntityBase) {
        self.ageable_ai_step();

        if self.has_stung() {
            let time = self.time_since_sting.fetch_add(1, Ordering::Relaxed) + 1;
            if time % 5 == 0 && rand::random_range(0..(1200 - time).clamp(1, 1200)) == 0 {
                caller.damage(
                    caller,
                    self.mob_entity.living_entity.get_max_health() + 1.0,
                    DamageType::GENERIC,
                );
            }
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::bee::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::bee::DATA_FLAGS_ID,
            self.flags.load(Ordering::Relaxed) as i8,
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack, Sound::EntityBeePollinate)
    }
}

fn bee(mob: &dyn Mob) -> Option<&BeeEntity> {
    mob.cast_any().downcast_ref::<BeeEntity>()
}
fn fly_to(mob: &dyn Mob, pos: Vector3<f64>, speed: f64) {
    mob.get_mob_entity()
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .set_progress(NavigatorGoal::new(mob.get_entity().pos.load(), pos, speed));
}
struct BeeStingGoal;
impl Goal for BeeStingGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        bee(mob).is_some_and(|b| !b.has_stung())
            && mob
                .get_mob_entity()
                .get_target()
                .is_some_and(|t| crate::entity::ai::goal::is_valid_attack_target(mob, t.as_ref()))
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob)
    }
    fn tick(&mut self, mob: &dyn Mob) {
        let (Some(b), Some(target)) = (bee(mob), mob.get_mob_entity().get_target()) else {
            return;
        };
        let entity = mob.get_entity();
        fly_to(mob, target.get_entity().pos.load(), 1.4);
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(&target, 30.0, 30.0);
        if mob.get_mob_entity().is_in_attack_range(target.as_ref())
            && mob.has_line_of_sight(target.get_entity())
        {
            let damage = mob
                .get_mob_entity()
                .living_entity
                .get_attribute_value(&Attributes::ATTACK_DAMAGE) as f32;
            if target.damage_with_context(
                target.as_ref(),
                damage,
                DamageType::STING,
                Some(entity.pos.load()),
                Some(mob),
                Some(mob),
            ) {
                if let Some(living) = target.get_living_entity() {
                    let duration = match entity.world.load().level_info.load().difficulty {
                        pumpkin_util::Difficulty::Normal => 200,
                        pumpkin_util::Difficulty::Hard => 360,
                        _ => 0,
                    };
                    if duration > 0 {
                        living.add_effect(Effect {
                            effect_type: &StatusEffect::POISON,
                            duration,
                            amplifier: 0,
                            ambient: false,
                            show_particles: true,
                            show_icon: true,
                            blend: false,
                        });
                    }
                    target.get_entity().set_synced_data(
                        pumpkin_data::tracked_data::living_entity::DATA_STINGER_COUNT_ID,
                        VarInt(1),
                    );
                }
                b.set_has_stung(true);
                b.mob_entity.anger.stop(b);
                entity.play_sound(Sound::EntityBeeSting);
            }
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
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
        Controls::MOVE | Controls::LOOK
    }
}
#[derive(Default)]
struct BeePollinateGoal {
    flower: Option<BlockPos>,
    ticks: i32,
    successful: i32,
    retry: i64,
}
impl Goal for BeePollinateGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(b) = bee(mob) else {
            return false;
        };
        let entity = mob.get_entity();
        let world = entity.world.load();
        if b.has_nectar()
            || b.has_stung()
            || world.is_raining()
            || b.wants_hive()
            || b.mob_entity.get_target().is_some()
            || world.get_world_age() < self.retry
        {
            return false;
        }
        self.retry = world.get_world_age() + rand::random_range(20..=60);
        let origin = entity.block_pos.load();
        let mut flowers = Vec::new();
        for x in -5..=5 {
            for z in -5..=5 {
                for y in -3..=3 {
                    let pos = origin.offset(Vector3::new(x, y, z));
                    let block = world.get_block(&pos);
                    if block.has_tag(&tag::Block::MINECRAFT_FLOWERS)
                        && !world.get_block_state(&pos).is_waterlogged()
                    {
                        flowers.push(pos);
                    }
                }
            }
        }
        flowers.sort_by(|a, b| {
            a.to_centered_f64()
                .squared_distance_to_vec(&entity.pos.load())
                .total_cmp(
                    &b.to_centered_f64()
                        .squared_distance_to_vec(&entity.pos.load()),
                )
        });
        self.flower = flowers.into_iter().find(|pos| {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .can_reach_within(
                    &mob.get_mob_entity().living_entity,
                    pos.to_f64() + Vector3::new(0.5, 0.6, 0.5),
                    1.0,
                )
        });
        self.flower.is_some()
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.ticks = 0;
        self.successful = 0;
        if let Some(b) = bee(mob) {
            b.flower_position.store(self.flower);
            b.ticks_without_nectar.store(0, Ordering::Relaxed);
        }
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.ticks < 600
            && self.successful < 400
            && self.flower.is_some_and(|p| {
                mob.get_entity()
                    .world
                    .load()
                    .get_block(&p)
                    .has_tag(&tag::Block::MINECRAFT_FLOWERS)
            })
            && !mob.get_entity().world.load().is_raining()
            && mob.get_mob_entity().get_target().is_none()
    }
    fn tick(&mut self, mob: &dyn Mob) {
        self.ticks += 1;
        let Some(flower) = self.flower else {
            return;
        };
        let hover = flower.to_f64() + Vector3::new(0.5, 0.6, 0.5);
        let entity = mob.get_entity();
        let delta = hover - entity.pos.load();
        if delta.length() < 1.0 {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .stop();
            mob.get_mob_entity()
                .living_entity
                .movement_input
                .store(Vector3::new(0.0, 0.0, 0.0));
            entity.set_velocity(delta * 0.1);
            self.successful += 1;
            if self.successful % 60 == 0 {
                entity.play_sound(Sound::EntityBeePollinate);
            }
        } else if self.ticks % 20 == 1 {
            fly_to(mob, hover, 1.2);
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(b) = bee(mob) {
            if self.successful >= 400 {
                b.set_has_nectar(true);
                b.crops_grown_since_pollination.store(0, Ordering::Relaxed);
            }
        }
        self.retry = mob.get_entity().world.load().get_world_age() + 200;
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
struct BeeHiveGoal {
    ticks: i32,
    retry: i64,
}
impl Goal for BeeHiveGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(b) = bee(mob) else {
            return false;
        };
        let entity = mob.get_entity();
        let world = entity.world.load();
        if !b.wants_hive() || world.get_world_age() < self.retry {
            return false;
        }
        if let Some(pos) = b.hive_position.load()
            && let Some(be) = world.get_block_entity(&pos)
            && let Some(hive) = be.as_any().downcast_ref::<BeehiveBlockEntity>()
            && hive.has_space()
        {
            return true;
        }
        self.retry = world.get_world_age() + 200;
        let mut hives: Vec<_> = world
            .block_entities
            .iter()
            .flat_map(|chunk| {
                chunk
                    .values()
                    .filter_map(|be| {
                        be.as_any()
                            .downcast_ref::<BeehiveBlockEntity>()
                            .filter(|hive| hive.has_space())
                            .map(|hive| hive.position)
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|pos| {
                pos.to_centered_f64()
                    .squared_distance_to_vec(&entity.pos.load())
                    <= 400.0
            })
            .collect();
        hives.sort_by(|a, b| {
            a.to_centered_f64()
                .squared_distance_to_vec(&entity.pos.load())
                .total_cmp(
                    &b.to_centered_f64()
                        .squared_distance_to_vec(&entity.pos.load()),
                )
        });
        b.hive_position.store(hives.first().copied());
        !hives.is_empty()
    }
    fn start(&mut self, _mob: &dyn Mob) {
        self.ticks = 0;
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.ticks < 1200
            && bee(mob).is_some_and(|b| b.wants_hive() && b.hive_position.load().is_some())
    }
    fn tick(&mut self, mob: &dyn Mob) {
        self.ticks += 1;
        let Some(b) = bee(mob) else {
            return;
        };
        let Some(pos) = b.hive_position.load() else {
            return;
        };
        let entity = mob.get_entity();
        let world = entity.world.load();
        let Some(be) = world.get_block_entity(&pos) else {
            b.hive_position.store(None);
            return;
        };
        let Some(hive) = be.as_any().downcast_ref::<BeehiveBlockEntity>() else {
            b.hive_position.store(None);
            return;
        };
        if !hive.has_space() {
            b.hive_position.store(None);
            return;
        }
        if pos
            .to_centered_f64()
            .squared_distance_to_vec(&entity.pos.load())
            < 4.0
        {
            hive.try_enter(b);
        } else if self.ticks % 20 == 1 {
            fly_to(mob, hive.entrance(&world), 1.0);
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
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
