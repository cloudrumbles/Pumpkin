use crate::{
    entity::{
        Entity, EntityBase,
        ai::{
            goal::{
                Controls, Goal, active_target::ActiveTargetGoal, is_valid_attack_target,
                melee_attack::MeleeAttackGoal, ranged_attack::RangedAttackGoal, swim::SwimGoal,
                zombie_attack::ZombieAttackGoal,
            },
            pathfinder::{Navigator, NavigatorGoal},
        },
        mob::{
            Mob, MobEntity, RangedAttackMob, equipment::RegionalDifficulty,
            zombie::ZombieEntityBase,
        },
        projectile::{arrow::ArrowPickup, trident::TridentEntity},
    },
    world::World,
};
use pumpkin_data::{
    data_component_impl::EquipmentSlot, entity::EntityType, fluid::Fluid, item::Item,
    item_stack::ItemStack, sound::Sound,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;
use std::sync::{Arc, atomic::Ordering};

pub struct DrownedEntity {
    entity: Arc<ZombieEntityBase>,
}

fn is_day(world: &World) -> bool {
    (0..12000).contains(&world.get_time_of_day().rem_euclid(24000))
}
fn valid_target(mob: &dyn Mob) -> bool {
    mob.get_mob_entity().get_target().is_some_and(|target| {
        is_valid_attack_target(mob, target.as_ref())
            && (!is_day(&mob.get_entity().world.load())
                || target.get_entity().touching_water.load(Ordering::Relaxed))
    })
}
fn has_trident(mob: &dyn Mob) -> bool {
    mob.get_mob_entity()
        .living_entity
        .entity_equipment
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&EquipmentSlot::MAIN_HAND)
        .item
        == &Item::TRIDENT
}

impl DrownedEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        Self::with_can_break_doors(entity, false)
    }

    pub fn with_can_break_doors(entity: Entity, _can_break_doors: bool) -> Arc<Self> {
        let drowned = Arc::new(Self {
            entity: ZombieEntityBase::new(entity),
        });
        let mob = &drowned.entity.mob_entity;
        *mob.navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Navigator::amphibious(false);
        let ranged: Arc<dyn RangedAttackMob> = drowned.clone();
        {
            let mut goals = mob
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            goals.remove_goals::<SwimGoal>();
            goals.remove_goals::<ZombieAttackGoal>();
            goals.add_goal(1, Box::new(DrownedHabitatGoal::new(true)));
            goals.add_goal(
                2,
                Box::new(DrownedTridentGoal(RangedAttackGoal::new(
                    Arc::downgrade(&ranged),
                    1.0,
                    40,
                    10.0,
                ))),
            );
            goals.add_goal(
                2,
                Box::new(DrownedMeleeGoal(MeleeAttackGoal::new(1.0, false))),
            );
            goals.add_goal(5, Box::new(DrownedHabitatGoal::new(false)));
        }
        {
            let mut targets = mob
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            targets.remove_goals::<ActiveTargetGoal>();
            targets.add_goal(
                2,
                ActiveTargetGoal::predicated(mob, 10, true, |target, world| {
                    let kind = target.entity.entity_type;
                    if kind == &EntityType::PLAYER {
                        !is_day(world) || target.entity.touching_water.load(Ordering::Relaxed)
                    } else if [
                        &EntityType::VILLAGER,
                        &EntityType::IRON_GOLEM,
                        &EntityType::AXOLOTL,
                    ]
                    .contains(&kind)
                    {
                        true
                    } else {
                        kind == &EntityType::TURTLE
                            && target.entity.age.load(Ordering::Relaxed) < 0
                            && !target.entity.touching_water.load(Ordering::Relaxed)
                    }
                }),
            );
        }
        drowned
    }
}

impl Mob for DrownedEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.entity.mob_entity
    }
    fn get_mob_gravity(&self) -> f64 {
        if self.get_entity().touching_water.load(Ordering::Relaxed)
            && (valid_target(self) || !is_day(&self.get_entity().world.load()))
        {
            0.0
        } else {
            self.get_mob_entity().living_entity.get_gravity()
        }
    }
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if self.get_mob_entity().get_target().is_some() && !valid_target(self) {
            self.get_mob_entity().set_target(None);
        }
    }
    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        _difficulty: &RegionalDifficulty,
    ) {
        if rand::random::<f32>() > 0.9 {
            let item = if rand::random_range(0..16) < 10 {
                &Item::TRIDENT
            } else {
                &Item::FISHING_ROD
            };
            self.get_mob_entity()
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .put(&EquipmentSlot::MAIN_HAND, ItemStack::new(1, item));
        }
    }
    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.entity.mob_write_nbt(nbt);
    }
    fn mob_read_nbt(&self, _nbt: &NbtCompound) {
        self.entity.set_can_break_doors(false, self);
    }
}

impl RangedAttackMob for DrownedEntity {
    fn perform_ranged_attack(&self, target: &Arc<dyn EntityBase>, _power: f32) {
        if !has_trident(self)
            || !is_valid_attack_target(self, target.as_ref())
            || !self.has_line_of_sight(target.get_entity())
        {
            return;
        }
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let stack = self
            .get_mob_entity()
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::MAIN_HAND);
        let trident = TridentEntity::new_shot(
            Entity::new(world.clone(), entity.pos.load(), &EntityType::TRIDENT),
            entity,
            stack.clone(),
            ArrowPickup::Disallowed,
        );
        let mut aim = target.get_entity().pos.load()
            + Vector3::new(0.0, f64::from(target.get_entity().height()) / 3.0, 0.0)
            - trident.entity.pos.load();
        aim.y += aim.horizontal_length() * 0.2;
        trident.set_velocity(
            aim.x,
            aim.y,
            aim.z,
            1.6,
            f64::from(14 - 4 * world.level_info.load().difficulty as i32),
        );
        trident.apply_on_projectile_spawned(&stack);
        world.spawn_entity(Arc::new(trident));
        entity.play_sound(Sound::EntityDrownedShoot);
    }
}

struct DrownedTridentGoal(RangedAttackGoal);
impl Goal for DrownedTridentGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        has_trident(mob) && valid_target(mob) && self.0.can_start(mob)
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        has_trident(mob) && valid_target(mob) && self.0.should_continue(mob)
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.0.start(mob);
        mob.get_mob_entity().set_attacking(true);
    }
    fn stop(&mut self, mob: &dyn Mob) {
        self.0.stop(mob);
        mob.get_mob_entity().set_attacking(false);
    }
    fn tick(&mut self, mob: &dyn Mob) {
        self.0.tick(mob);
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        self.0.controls()
    }
}
struct DrownedMeleeGoal(MeleeAttackGoal);
impl Goal for DrownedMeleeGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        valid_target(mob) && self.0.can_start(mob)
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        valid_target(mob) && self.0.should_continue(mob)
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.0.start(mob);
    }
    fn stop(&mut self, mob: &dyn Mob) {
        self.0.stop(mob);
    }
    fn tick(&mut self, mob: &dyn Mob) {
        self.0.tick(mob);
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        self.0.controls()
    }
}

struct DrownedHabitatGoal {
    day: bool,
    destination: Option<Vector3<f64>>,
    remaining: i32,
    next_search: i64,
}
impl DrownedHabitatGoal {
    fn new(day: bool) -> Self {
        Self {
            day,
            destination: None,
            remaining: 0,
            next_search: 0,
        }
    }
}
impl Goal for DrownedHabitatGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        let world = entity.world.load();
        if is_day(&world) != self.day
            || world.get_world_age() < self.next_search
            || (!self.day && mob.get_mob_entity().get_target().is_some())
        {
            return false;
        }
        let wet = entity.touching_water.load(Ordering::Relaxed);
        if wet == self.day {
            return false;
        }
        self.next_search = world.get_world_age() + 40;
        let origin = entity.block_pos.load();
        self.destination = None;
        let mut candidates = Vec::new();
        if !self.day && entity.pos.load().y < f64::from(world.sea_level - 2) {
            for _ in 0..16 {
                let block = origin.offset(Vector3::new(
                    rand::random_range(-4..=4),
                    rand::random_range(1..=8),
                    rand::random_range(-4..=4),
                ));
                if world.get_fluid(&block).matches_type(&Fluid::WATER) {
                    candidates.push(Vector3::new(
                        f64::from(block.0.x) + 0.5,
                        f64::from(block.0.y).min(f64::from(world.sea_level - 1)),
                        f64::from(block.0.z) + 0.5,
                    ));
                }
            }
        } else {
            for _ in 0..32 {
                let block = origin.offset(Vector3::new(
                    rand::random_range(-10..=10),
                    rand::random_range(-5..=2),
                    rand::random_range(-10..=10),
                ));
                let valid = if self.day {
                    world.get_fluid(&block).matches_type(&Fluid::WATER)
                } else {
                    world.get_block(&block.down()).is_solid()
                        && world.get_block(&block).is_air()
                        && world.get_block(&block.up()).is_air()
                };
                if valid {
                    candidates.push(Vector3::new(
                        f64::from(block.0.x) + 0.5,
                        f64::from(block.0.y),
                        f64::from(block.0.z) + 0.5,
                    ));
                }
            }
        }
        candidates.sort_by(|a, b| {
            entity
                .pos
                .load()
                .squared_distance_to_vec(a)
                .total_cmp(&entity.pos.load().squared_distance_to_vec(b))
        });
        self.destination = candidates.into_iter().find(|pos| {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .can_reach_within(&mob.get_mob_entity().living_entity, *pos, 1.0)
        });
        self.destination.is_some()
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.remaining = 160;
        if let Some(destination) = self.destination {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_progress(NavigatorGoal::new(
                    mob.get_entity().pos.load(),
                    destination,
                    1.0,
                ));
        }
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.remaining > 0
            && is_day(&mob.get_entity().world.load()) == self.day
            && !mob.is_navigator_idle()
    }
    fn tick(&mut self, _mob: &dyn Mob) {
        self.remaining -= 1;
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
