use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::{
        goal::{
            Controls, Goal,
            aquatic::{RandomSwimGoal, aquatic_gravity, configure_swimming},
            avoid_entity::AvoidEntityGoal,
            look_around::RandomLookAroundGoal,
            look_at_entity::LookAtEntityGoal,
            melee_attack::MeleeAttackGoal,
            revenge::RevengeGoal,
            try_find_water::TryFindWaterGoal,
        },
        pathfinder::NavigatorGoal,
    },
    item::ItemEntity,
    mob::{Mob, MobEntity},
    player::Player,
};
use crossbeam::atomic::AtomicCell;
use pumpkin_data::{
    damage::DamageType,
    data_component_impl::EquipmentSlot,
    effect::StatusEffect,
    entity::{EntityStatus, EntityType},
    fluid::Fluid,
    item_stack::ItemStack,
    potion::Effect,
    sound::Sound,
    tag::{self, Taggable},
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

pub struct DolphinEntity {
    pub mob_entity: MobEntity,
    ageable: AgeableData,
    pub got_fish: AtomicBool,
    pub moistness_level: AtomicI32,
    pub air_supply: AtomicI32,
    treasure: AtomicCell<Option<BlockPos>>,
    searching: AtomicBool,
    search_done: AtomicBool,
}

fn navigate(mob: &dyn Mob, point: Vector3<f64>, speed: f64) {
    mob.get_mob_entity()
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .set_progress(NavigatorGoal::new(
            mob.get_entity().pos.load(),
            point,
            speed,
        ));
}
fn stop(mob: &dyn Mob) {
    mob.get_mob_entity()
        .navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .stop();
}

impl DolphinEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        configure_swimming(&mob_entity);
        let dolphin = Arc::new(Self {
            mob_entity,
            ageable: AgeableData::default(),
            got_fish: AtomicBool::new(false),
            moistness_level: AtomicI32::new(2400),
            air_supply: AtomicI32::new(4800),
            treasure: AtomicCell::new(None),
            searching: AtomicBool::new(false),
            search_done: AtomicBool::new(false),
        });
        let mob: Arc<dyn Mob> = dolphin.clone();
        let weak = Arc::downgrade(&dolphin);
        {
            let mut goals = dolphin
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            goals.add_goal(
                0,
                Box::new(DolphinBreathGoal {
                    dolphin: weak.clone(),
                    destination: None,
                }),
            );
            goals.add_goal(0, Box::new(TryFindWaterGoal));
            goals.add_goal(
                1,
                Box::new(DolphinTreasureGoal {
                    dolphin: weak,
                    next_path: 0,
                    stuck: 0,
                }),
            );
            goals.add_goal(2, Box::<DolphinSwimWithPlayerGoal>::default());
            goals.add_goal(4, Box::new(RandomSwimGoal::new(1.0, 10)));
            goals.add_goal(4, Box::new(RandomLookAroundGoal::default()));
            goals.add_goal(
                5,
                LookAtEntityGoal::with_default(Arc::downgrade(&mob), &EntityType::PLAYER, 6.0),
            );
            goals.add_goal(5, Box::<DolphinJumpGoal>::default());
            goals.add_goal(6, Box::new(MeleeAttackGoal::new(1.2, true)));
            goals.add_goal(7, Box::<DolphinPlayGoal>::default());
            goals.add_goal(
                9,
                Box::new(AvoidEntityGoal::new(&EntityType::GUARDIAN, 8.0, 1.0, 1.0)),
            );
        }
        dolphin
            .mob_entity
            .target_selector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .add_goal(1, Box::new(RevengeGoal::new(true)));
        dolphin
    }
    pub fn got_fish(&self) -> bool {
        self.got_fish.load(Ordering::Relaxed)
    }
    pub fn set_got_fish(&self, value: bool) {
        if self.got_fish.swap(value, Ordering::Relaxed) != value && value {
            self.treasure.store(None);
            self.search_done.store(false, Ordering::Relaxed);
        }
        self.get_entity()
            .set_synced_data(pumpkin_data::tracked_data::dolphin::GOT_FISH, value);
    }
    fn update_air(&self, value: i32) {
        self.air_supply.store(value, Ordering::Relaxed);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::entity::DATA_AIR_SUPPLY_ID,
            VarInt(value),
        );
    }
}
impl AgeableMob for DolphinEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable
    }
}
impl Mob for DolphinEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }
    fn get_mob_gravity(&self) -> f64 {
        aquatic_gravity(&self.mob_entity)
    }
    fn mob_tick(&self, caller: &dyn EntityBase) {
        self.ageable_ai_step();
        if !self.mob_entity.living_entity.is_alive() {
            return;
        }
        if self.mob_entity.is_no_ai() {
            if self.air_supply.load(Ordering::Relaxed) != 4800 {
                self.update_air(4800);
            }
            return;
        }
        let entity = self.get_entity();
        let world = entity.world.load();
        if entity.is_submerged_in_water()
            && !self
                .mob_entity
                .living_entity
                .has_effect(&StatusEffect::WATER_BREATHING)
        {
            let air = self.air_supply.fetch_sub(1, Ordering::Relaxed) - 1;
            if air <= -20 {
                self.update_air(0);
                caller.damage(caller, 2.0, DamageType::DROWN);
            } else if air % 20 == 0 {
                self.update_air(air);
            }
        } else if self.air_supply.load(Ordering::Relaxed) != 4800 {
            self.update_air(4800);
        }
        let wet = entity.touching_water.load(Ordering::Relaxed)
            || world.is_raining_at(&entity.block_pos.load());
        let moisture = if wet {
            2400
        } else {
            self.moistness_level
                .load(Ordering::Relaxed)
                .saturating_sub(1)
        };
        self.moistness_level.store(moisture, Ordering::Relaxed);
        if entity.age.load(Ordering::Relaxed) % 20 == 0 {
            entity.set_synced_data(
                pumpkin_data::tracked_data::dolphin::MOISTNESS_LEVEL,
                VarInt(moisture),
            );
        }
        if !wet {
            if moisture <= 0 {
                caller.damage(caller, 1.0, DamageType::DRY_OUT);
            }
            if entity.on_ground.load(Ordering::Relaxed) {
                entity.set_velocity(
                    entity.velocity.load()
                        + Vector3::new(
                            rand::random_range(-0.2..0.2),
                            0.5,
                            rand::random_range(-0.2..0.2),
                        ),
                );
                entity.yaw.store(rand::random_range(0.0..360.0));
                entity.on_ground.store(false, Ordering::Relaxed);
            }
        }
    }
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if self.is_navigator_idle()
            && self.mob_entity.get_target().is_none()
            && self.get_entity().touching_water.load(Ordering::Relaxed)
        {
            let mut velocity = self.get_entity().velocity.load();
            velocity.y -= 0.005;
            self.get_entity().set_velocity(velocity);
        }
    }
    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("GotFish", self.got_fish());
        nbt.put_int("Moistness", self.moistness_level.load(Ordering::Relaxed));
        nbt.put_short("Air", self.air_supply.load(Ordering::Relaxed) as i16);
    }
    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.set_got_fish(nbt.get_bool("GotFish").unwrap_or(false));
        self.moistness_level
            .store(nbt.get_int("Moistness").unwrap_or(2400), Ordering::Relaxed);
        self.update_air(nbt.get_short("Air").map_or(4800, i32::from));
    }
    fn mob_init_data_tracker(&self) {
        self.set_got_fish(self.got_fish());
        self.update_air(self.air_supply.load(Ordering::Relaxed));
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::dolphin::MOISTNESS_LEVEL,
            VarInt(self.moistness_level.load(Ordering::Relaxed)),
        );
    }
    fn mob_interact(&self, player: &Arc<Player>, stack: &mut ItemStack) -> bool {
        if !stack.is_empty() && stack.item.has_tag(&tag::Item::MINECRAFT_FISHES) {
            stack.decrement_unless_creative(player.gamemode.load(), 1);
            if self.can_age_up() {
                self.age_up(
                    Self::get_speed_up_seconds_when_feeding(-self.get_age()),
                    true,
                );
            } else {
                self.set_got_fish(true);
            }
            self.get_entity().play_sound(Sound::EntityDolphinEat);
            return true;
        }
        false
    }
}

struct DolphinBreathGoal {
    dolphin: Weak<DolphinEntity>,
    destination: Option<Vector3<f64>>,
}
impl Goal for DolphinBreathGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        self.dolphin
            .upgrade()
            .is_some_and(|d| d.air_supply.load(Ordering::Relaxed) < 140)
    }
    fn start(&mut self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let origin = entity.block_pos.load();
        self.destination = None;
        'search: for y in 0..=8 {
            for x in -1..=1 {
                for z in -1..=1 {
                    let block = origin.offset(Vector3::new(x, y, z));
                    if world.get_block(&block).is_air()
                        && world.get_fluid(&block.down()).matches_type(&Fluid::WATER)
                    {
                        self.destination = Some(Vector3::new(
                            f64::from(block.0.x) + 0.5,
                            f64::from(block.0.y) - f64::from(entity.get_eye_height()) + 0.05,
                            f64::from(block.0.z) + 0.5,
                        ));
                        break 'search;
                    }
                }
            }
        }
        if self.destination.is_none() {
            self.destination = Some(entity.pos.load() + Vector3::new(0.0, 1.0, 0.0));
        }
        if let Some(point) = self.destination {
            navigate(mob, point, 1.0);
        }
    }
    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        self.dolphin
            .upgrade()
            .is_some_and(|d| d.air_supply.load(Ordering::Relaxed) < 4800)
    }
    fn tick(&mut self, mob: &dyn Mob) {
        if mob.is_navigator_idle() {
            self.start(mob);
        }
        if let Some(point) = self.destination {
            let entity = mob.get_entity();
            let delta = point - entity.pos.load();
            if delta.y > 0.0 && delta.x * delta.x + delta.z * delta.z < 2.0 {
                let mut velocity = entity.velocity.load();
                velocity.y = velocity.y.max(0.08);
                entity.set_velocity(velocity);
            }
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        stop(mob);
        self.destination = None;
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

struct DolphinTreasureGoal {
    dolphin: Weak<DolphinEntity>,
    next_path: i64,
    stuck: i32,
}
impl Goal for DolphinTreasureGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        self.dolphin
            .upgrade()
            .is_some_and(|d| d.got_fish() && d.air_supply.load(Ordering::Relaxed) >= 140)
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.stuck = 0;
        self.next_path = 0;
        stop(mob);
        let Some(dolphin) = self.dolphin.upgrade() else {
            return;
        };
        if dolphin.treasure.load().is_some() || dolphin.searching.swap(true, Ordering::AcqRel) {
            return;
        }
        let weak = self.dolphin.clone();
        let world = mob.get_entity().world.load_full();
        let origin = mob.get_entity().block_pos.load();
        // Structure generation is expensive. Never perform the search on the tick thread.
        std::thread::spawn(move || {
            use pumpkin_data::structures::{StructureKeys, StructureSet};
            use pumpkin_world::generation::generator::structure_finder::find_nearest_structure_start;
            let generator = world.level.world_gen();
            let targets = [
                (
                    "ocean_ruins",
                    vec![StructureKeys::OceanRuinCold, StructureKeys::OceanRuinWarm],
                ),
                (
                    "shipwrecks",
                    vec![StructureKeys::Shipwreck, StructureKeys::ShipwreckBeached],
                ),
            ];
            let found = targets
                .iter()
                .filter_map(|(set, keys)| {
                    StructureSet::get(set).and_then(|set| {
                        find_nearest_structure_start(origin, set, keys, 50, &generator)
                    })
                })
                .min_by_key(|pos| {
                    let x = i64::from(pos.0.x) - i64::from(origin.0.x);
                    let z = i64::from(pos.0.z) - i64::from(origin.0.z);
                    x * x + z * z
                });
            if let Some(d) = weak.upgrade() {
                d.treasure.store(found);
                d.search_done.store(true, Ordering::Release);
                d.searching.store(false, Ordering::Release);
            }
        });
    }
    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        self.dolphin.upgrade().is_some_and(|d| {
            d.got_fish()
                && d.air_supply.load(Ordering::Relaxed) >= 100
                && self.stuck < 8
                && (!d.search_done.load(Ordering::Acquire)
                    || d.treasure.load().is_some_and(|p| {
                        let at = d.get_entity().pos.load();
                        let dx = at.x - f64::from(p.0.x);
                        let dz = at.z - f64::from(p.0.z);
                        dx * dx + dz * dz >= 16.0
                    }))
        })
    }
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(d) = self.dolphin.upgrade() else {
            return;
        };
        let Some(target) = d.treasure.load() else {
            return;
        };
        let entity = mob.get_entity();
        let world = entity.world.load();
        let now = world.get_world_age();
        if now < self.next_path {
            return;
        }
        self.next_path = now + 20;
        let pos = entity.pos.load();
        let delta = Vector3::new(
            f64::from(target.0.x) - pos.x,
            0.0,
            f64::from(target.0.z) - pos.z,
        );
        let direction = delta.normalize();
        let mut destination = None;
        for distance in [16.0_f64, 8.0, 4.0] {
            for dy in [0.0, 1.0, -1.0, 2.0, -2.0] {
                let point =
                    pos + direction * distance.min(delta.length()) + Vector3::new(0.0, dy, 0.0);
                if world
                    .get_fluid(&BlockPos::floored_v(point))
                    .matches_type(&Fluid::WATER)
                    && mob
                        .get_mob_entity()
                        .navigator
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .can_reach_within(&mob.get_mob_entity().living_entity, point, 1.0)
                {
                    destination = Some(point);
                    break;
                }
            }
            if destination.is_some() {
                break;
            }
        }
        if let Some(point) = destination {
            navigate(mob, point, 1.3);
            self.stuck = 0;
            world.send_entity_status(entity, EntityStatus::DolphinLookingForTreasure, None);
        } else {
            self.stuck += 1;
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(d) = self.dolphin.upgrade()
            && d.air_supply.load(Ordering::Relaxed) >= 140
        {
            d.set_got_fish(false);
        }
        stop(mob);
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

#[derive(Default)]
struct DolphinSwimWithPlayerGoal {
    player: Option<Arc<Player>>,
    ticks: i32,
}
impl DolphinSwimWithPlayerGoal {
    fn effect(player: &Player) {
        let effect = Effect {
            effect_type: &StatusEffect::DOLPHINS_GRACE,
            duration: 100,
            amplifier: 0,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: false,
        };
        player.living_entity.add_effect(effect.clone());
        player.send_effect(&effect);
    }
}
impl Goal for DolphinSwimWithPlayerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if !mob.get_entity().touching_water.load(Ordering::Relaxed) {
            return false;
        }
        let target_id = mob
            .get_mob_entity()
            .get_target()
            .map(|e| e.get_entity().entity_id);
        self.player = mob.get_entity().world.load().get_nearest_player(
            mob.get_entity().pos.load(),
            10.0,
            |p| {
                !p.is_spectator()
                    && p.get_entity().is_swimming()
                    && Some(p.get_entity().entity_id) != target_id
            },
        );
        self.player.is_some()
    }
    fn start(&mut self, _mob: &dyn Mob) {
        self.ticks = 0;
        if let Some(p) = &self.player {
            Self::effect(p);
        }
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.player.as_ref().is_some_and(|p| {
            !p.is_spectator()
                && p.get_entity().is_swimming()
                && p.living_entity.is_alive()
                && p.get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&mob.get_entity().pos.load())
                    < 256.0
        })
    }
    fn tick(&mut self, mob: &dyn Mob) {
        self.ticks += 1;
        let Some(player) = &self.player else {
            return;
        };
        if self.ticks % 6 == 0 {
            Self::effect(player);
        }
        let target: Arc<dyn EntityBase> = player.clone();
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(&target, 45.0, 85.0);
        if target
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&mob.get_entity().pos.load())
            < 6.25
        {
            stop(mob);
        } else if self.ticks % 10 == 1 {
            navigate(mob, target.get_entity().pos.load(), 4.0);
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        self.player = None;
        stop(mob);
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

#[derive(Default)]
struct DolphinJumpGoal {
    ticks: i32,
    left_water: bool,
    direction: Option<Vector3<f64>>,
}
impl Goal for DolphinJumpGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load();
        if !entity.touching_water.load(Ordering::Relaxed) || rand::random_range(0..10) != 0 {
            return false;
        }
        let yaw = f64::from(entity.yaw.load()).to_radians();
        let direction = Vector3::new(-yaw.sin(), 0.0, yaw.cos());
        let pos = entity.pos.load();
        if [0.0, 1.0, 4.0, 5.0, 6.0, 7.0].iter().any(|distance| {
            let block = BlockPos::floored_v(pos + direction * *distance);
            !world.get_fluid(&block).matches_type(&Fluid::WATER)
                || !world.get_block(&block.up()).is_air()
                || !world.get_block(&block.up().up()).is_air()
        }) {
            return false;
        }
        self.direction = Some(direction);
        true
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.ticks = 0;
        self.left_water = false;
        stop(mob);
        if let Some(direction) = self.direction {
            mob.get_entity()
                .set_velocity(direction * 0.6 + Vector3::new(0.0, 0.7, 0.0));
        }
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.ticks < 60
            && (!self.left_water || !mob.get_entity().touching_water.load(Ordering::Relaxed))
    }
    fn tick(&mut self, mob: &dyn Mob) {
        self.ticks += 1;
        if !mob.get_entity().touching_water.load(Ordering::Relaxed) && !self.left_water {
            self.left_water = true;
            mob.get_entity().play_sound(Sound::EntityDolphinJump);
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        mob.get_entity().pitch.store(0.0);
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::JUMP
    }
}

#[derive(Default)]
struct DolphinPlayGoal {
    target: Option<Arc<dyn EntityBase>>,
    cooldown: i64,
    ticks: i32,
}
impl DolphinPlayGoal {
    fn throw_item(&self, mob: &dyn Mob) {
        let living = &mob.get_mob_entity().living_entity;
        let stack = living
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .put(&EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone());
        if stack.is_empty() {
            return;
        }
        let entity = mob.get_entity();
        let world = entity.world.load_full();
        let yaw = f64::from(entity.yaw.load()).to_radians();
        let pitch = f64::from(entity.pitch.load()).to_radians();
        let velocity = Vector3::new(
            -yaw.sin() * pitch.cos() * 0.3,
            pitch.sin() * 0.45,
            yaw.cos() * pitch.cos() * 0.3,
        );
        let item = ItemEntity::new_with_velocity(
            Entity::new(
                world.clone(),
                entity.pos.load()
                    + Vector3::new(0.0, f64::from(entity.get_eye_height()) - 0.3, 0.0),
                &EntityType::ITEM,
            ),
            stack,
            velocity,
            40,
        );
        world.spawn_entity(Arc::new(item));
        living.send_equipment_changes(&[(EquipmentSlot::MAIN_HAND, ItemStack::EMPTY.clone())]);
    }
}
impl Goal for DolphinPlayGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load();
        if world.get_world_age() < self.cooldown
            || !entity.touching_water.load(Ordering::Relaxed)
            || !world.level_info.load().game_rules.mob_griefing
        {
            return false;
        }
        let held = mob
            .get_mob_entity()
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND);
        if !held.is_empty() {
            return true;
        }
        self.target =
            world.get_nearest_entity(entity.pos.load(), 8.0, Some(&[&EntityType::ITEM]), |t| {
                t.get_entity().touching_water.load(Ordering::Relaxed)
                    && t.get_item_entity()
                        .is_some_and(|i| i.get_pickup_delay() == 0)
            });
        self.target.is_some()
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.ticks = 0;
        mob.get_entity().play_sound(Sound::EntityDolphinPlay);
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.ticks < 120 && mob.get_entity().touching_water.load(Ordering::Relaxed)
    }
    fn tick(&mut self, mob: &dyn Mob) {
        self.ticks += 1;
        let living = &mob.get_mob_entity().living_entity;
        if !living
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND)
            .is_empty()
        {
            self.throw_item(mob);
            self.ticks = 120;
            return;
        }
        let Some(target) = &self.target else {
            self.ticks = 120;
            return;
        };
        let Some(item) = target.get_item_entity() else {
            return;
        };
        if mob
            .get_entity()
            .bounding_box
            .load()
            .expand(1.0, 0.5, 1.0)
            .intersects(&target.get_entity().bounding_box.load())
        {
            if let Some(stack) = item.take_for_mob(living, u8::MAX, |_| true) {
                living
                    .entity_equipment
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .put(&EquipmentSlot::MAIN_HAND, stack.clone());
                living
                    .equipment_drop_chances
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(EquipmentSlot::MAIN_HAND, 2.0);
                living.send_equipment_changes(&[(EquipmentSlot::MAIN_HAND, stack)]);
            }
        } else if self.ticks % 10 == 1 {
            navigate(mob, target.get_entity().pos.load(), 1.2);
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        self.throw_item(mob);
        self.target = None;
        self.cooldown = mob.get_entity().world.load().get_world_age() + rand::random_range(0..100);
        stop(mob);
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}
