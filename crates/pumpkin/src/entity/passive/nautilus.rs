use crossbeam::atomic::AtomicCell;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering},
};
use uuid::Uuid;

use pumpkin_data::{
    damage::DamageType,
    effect::StatusEffect,
    entity::EntityStatus,
    item_stack::ItemStack,
    particle::Particle,
    potion::Effect,
    sound::{Sound, SoundCategory},
    tag::{self, Taggable},
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        Controls, Goal,
        aquatic::{RandomSwimGoal, aquatic_gravity, configure_swimming},
        breed::BreedGoal,
        is_valid_attack_target,
        tempt::TemptGoal,
    },
    custom_sound::CustomSound,
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

pub struct NautilusEntity {
    pub mob_entity: MobEntity,
    pub is_tame: AtomicBool,
    pub owner: AtomicCell<Option<Uuid>>,
    pub is_dashing: AtomicBool,
    pub dash_cooldown: AtomicI32,
    pub is_saddled: AtomicBool,
    pub inventory: Mutex<Vec<ItemStack>>,
    ageable_data: AgeableData,
    anger_until: AtomicI64,
    next_hunt: AtomicI64,
}

impl NautilusEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        configure_swimming(&mob_entity);
        let next_hunt = mob_entity.living_entity.entity.world.load().get_world_age()
            + rand::random_range(2400..=3600);
        let nautilus = Self {
            mob_entity,
            is_tame: AtomicBool::new(false),
            owner: AtomicCell::new(None),
            is_dashing: AtomicBool::new(false),
            dash_cooldown: AtomicI32::new(0),
            is_saddled: AtomicBool::new(false),
            inventory: Mutex::new(vec![ItemStack::new(0, &pumpkin_data::item::Item::AIR); 9]),
            ageable_data: AgeableData::default(),
            anger_until: AtomicI64::new(0),
            next_hunt: AtomicI64::new(next_hunt),
        };
        let nautilus = Arc::new(nautilus);
        {
            let mut goals = nautilus
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !nautilus.is_zombie() {
                goals.add_goal(0, BreedGoal::new(0.4));
            }
            goals.add_goal(
                1,
                Box::new(TemptGoal::with_tag(
                    if nautilus.is_zombie() { 0.9 } else { 1.3 },
                    &tag::Item::MINECRAFT_NAUTILUS_FOOD,
                    3.5,
                )),
            );
            goals.add_goal(2, Box::new(NautilusChargeGoal::default()));
            goals.add_goal(5, Box::new(RandomSwimGoal::new(1.0, 40)));
        }
        nautilus
    }

    fn is_zombie(&self) -> bool {
        self.get_entity().entity_type.id == pumpkin_data::entity::EntityType::ZOMBIE_NAUTILUS.id
    }

    pub fn is_dashing(&self) -> bool {
        self.is_dashing.load(Ordering::Relaxed)
    }

    pub fn set_dashing(&self, dashing: bool) {
        self.is_dashing.store(dashing, Ordering::Relaxed);
        self.mob_entity
            .living_entity
            .entity
            .set_synced_data(pumpkin_data::tracked_data::nautilus::DASH, dashing);
    }

    pub fn is_tame(&self) -> bool {
        self.is_tame.load(Ordering::Relaxed)
    }

    pub fn set_tame(&self, tame: bool, owner: Option<Uuid>) {
        self.is_tame.store(tame, Ordering::Relaxed);
        self.owner.store(owner);
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::nautilus::DATA_FLAGS_ID,
            if tame { 4i8 } else { 0i8 },
        );
        if tame {
            self.mob_entity.set_target(None);
        }
    }

    pub fn get_ambient_sound(&self) -> Sound {
        let is_baby = self
            .mob_entity
            .living_entity
            .entity
            .age
            .load(Ordering::Relaxed)
            < 0;
        let is_water = self
            .mob_entity
            .living_entity
            .entity
            .touching_water
            .load(Ordering::Relaxed);
        if is_baby {
            if is_water {
                Sound::EntityBabyNautilusAmbient
            } else {
                Sound::EntityBabyNautilusAmbientLand
            }
        } else if is_water {
            Sound::EntityNautilusAmbient
        } else {
            Sound::EntityNautilusAmbientLand
        }
    }

    pub fn get_dash_sound(&self) -> Sound {
        let is_water = self
            .mob_entity
            .living_entity
            .entity
            .touching_water
            .load(Ordering::Relaxed);
        if is_water {
            Sound::EntityNautilusDash
        } else {
            Sound::EntityNautilusDashLand
        }
    }

    pub fn get_dash_ready_sound(&self) -> Sound {
        let is_water = self
            .mob_entity
            .living_entity
            .entity
            .touching_water
            .load(Ordering::Relaxed);
        if is_water {
            Sound::EntityNautilusDashReady
        } else {
            Sound::EntityNautilusDashReadyLand
        }
    }

    pub fn get_eat_sound(&self) -> Sound {
        let is_baby = self
            .mob_entity
            .living_entity
            .entity
            .age
            .load(Ordering::Relaxed)
            < 0;
        if is_baby {
            Sound::EntityBabyNautilusEat
        } else {
            Sound::EntityNautilusEat
        }
    }

    pub fn get_swim_sound(&self) -> Sound {
        let is_baby = self
            .mob_entity
            .living_entity
            .entity
            .age
            .load(Ordering::Relaxed)
            < 0;
        if is_baby {
            Sound::EntityBabyNautilusSwim
        } else {
            Sound::EntityNautilusSwim
        }
    }
}

impl Animal for NautilusEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_NAUTILUS_FOOD)
    }
}

impl AgeableMob for NautilusEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

/// A committed straight-line charge, shared by living and zombie nautiluses.
/// Vanilla's ChargeAttack stops after a hit, 12 blocks, or loss of its target.
#[derive(Default)]
struct NautilusChargeGoal {
    direction: Vector3<f64>,
    origin: Vector3<f64>,
    cooldown_until: i64,
    ends_at: i64,
    hit: bool,
}

impl Goal for NautilusChargeGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(nautilus) = mob.cast_any().downcast_ref::<NautilusEntity>() else {
            return false;
        };
        let entity = nautilus.get_entity();
        let world = entity.world.load();
        let now = world.get_world_age();
        if now < self.cooldown_until
            || nautilus.is_tame()
            || entity.age.load(Ordering::Relaxed) < 0
            || !entity.touching_water.load(Ordering::Relaxed)
            || nautilus.mob_entity.is_in_love()
            || !entity
                .passengers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        {
            return false;
        }
        if nautilus.anger_until.load(Ordering::Relaxed) <= now {
            nautilus.mob_entity.set_target(None);
            if now >= nautilus.next_hunt.load(Ordering::Relaxed) {
                nautilus
                    .next_hunt
                    .store(now + rand::random_range(2400..=3600), Ordering::Relaxed);
                if rand::random::<bool>() {
                    let target =
                        world.get_nearest_entity(entity.pos.load(), 11.0, None, |candidate| {
                            candidate
                                .get_entity()
                                .entity_type
                                .has_tag(&tag::EntityType::MINECRAFT_NAUTILUS_HOSTILES)
                                && candidate
                                    .get_entity()
                                    .touching_water
                                    .load(Ordering::Relaxed)
                                && is_valid_attack_target(mob, candidate.as_ref())
                                && mob.has_line_of_sight(candidate.get_entity())
                        });
                    nautilus.mob_entity.set_target(target);
                }
            }
        }
        nautilus.mob_entity.get_target().is_some_and(|target| {
            is_valid_attack_target(mob, target.as_ref())
                && target.get_entity().touching_water.load(Ordering::Relaxed)
                && entity
                    .pos
                    .load()
                    .squared_distance_to_vec(&target.get_entity().pos.load())
                    < 121.0
                && mob.has_line_of_sight(target.get_entity())
        })
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(nautilus) = mob.cast_any().downcast_ref::<NautilusEntity>() else {
            return;
        };
        let Some(target) = nautilus.mob_entity.get_target() else {
            return;
        };
        self.origin = nautilus.get_entity().pos.load();
        self.direction = (target.get_entity().pos.load() - self.origin).normalize()
            * if nautilus.is_zombie() { 0.5 } else { 0.6 };
        self.ends_at = nautilus.get_entity().world.load().get_world_age() + 60;
        self.hit = false;
        nautilus
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        nautilus
            .mob_entity
            .living_entity
            .movement_input
            .store(Vector3::new(0.0, 0.0, 0.0));
        nautilus.get_entity().play_sound(nautilus.get_dash_sound());
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(nautilus) = mob.cast_any().downcast_ref::<NautilusEntity>() else {
            return false;
        };
        let entity = nautilus.get_entity();
        !self.hit
            && !nautilus.is_tame()
            && entity.touching_water.load(Ordering::Relaxed)
            && entity.world.load().get_world_age() < self.ends_at
            && entity.pos.load().squared_distance_to_vec(&self.origin) < 144.0
            && nautilus.mob_entity.get_target().is_some_and(|target| {
                is_valid_attack_target(mob, target.as_ref())
                    && target.get_entity().touching_water.load(Ordering::Relaxed)
                    && entity
                        .pos
                        .load()
                        .squared_distance_to_vec(&target.get_entity().pos.load())
                        < 121.0
                    && mob.has_line_of_sight(target.get_entity())
            })
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let m = mob.get_mob_entity();
        let entity = &m.living_entity.entity;
        entity.velocity.store(self.direction);
        if let Some(target) = m.get_target() {
            let p = target.get_entity().pos.load();
            m.look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at_with_range(p.x, target.get_entity().get_eye_y(), p.z, 360.0, 360.0);
        }
        let world = entity.world.load();
        for target in world.get_all_at_box(&entity.bounding_box.load()) {
            if is_valid_attack_target(mob, target.as_ref()) {
                m.try_attack(mob, target.as_ref());
                target
                    .get_entity()
                    .knockback(0.8, -self.direction.x, -self.direction.z);
                self.hit = true;
                break;
            }
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.cooldown_until = mob
            .get_mob_entity()
            .living_entity
            .entity
            .world
            .load()
            .get_world_age()
            + 80;
        mob.get_mob_entity().set_target(None);
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}

impl CustomSound for NautilusEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        let is_water = entity.touching_water.load(Ordering::Relaxed);
        Some(if is_baby {
            if is_water {
                Sound::EntityBabyNautilusHurt
            } else {
                Sound::EntityBabyNautilusHurtLand
            }
        } else if is_water {
            Sound::EntityNautilusHurt
        } else {
            Sound::EntityNautilusHurtLand
        })
    }

    fn death_sound(&self) -> Option<Sound> {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        let is_water = entity.touching_water.load(Ordering::Relaxed);
        Some(if is_baby {
            if is_water {
                Sound::EntityBabyNautilusDeath
            } else {
                Sound::EntityBabyNautilusDeathLand
            }
        } else if is_water {
            Sound::EntityNautilusDeath
        } else {
            Sound::EntityNautilusDeathLand
        })
    }
}

impl Mob for NautilusEntity {
    fn as_custom_sound(&self) -> Option<&dyn crate::entity::custom_sound::CustomSound> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        (!self.is_zombie()).then_some(self)
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        (!self.is_zombie()).then_some(self)
    }

    fn requires_custom_persistence(&self) -> bool {
        self.is_tame()
    }

    fn get_mob_gravity(&self) -> f64 {
        aquatic_gravity(&self.mob_entity)
    }

    fn on_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) {
        if !self.is_tame()
            && let Some(attacker) = source
            && is_valid_attack_target(self, attacker)
        {
            let world = self.get_entity().world.load();
            self.anger_until
                .store(world.get_world_age() + 400, Ordering::Relaxed);
            self.mob_entity
                .set_target(world.get_entity_by_uuid(attacker.get_entity().entity_uuid));
        }
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        if !self.is_zombie() {
            self.write_ageable_nbt(nbt);
            self.write_animal_nbt(nbt);
        }
        nbt.put_bool("IsTame", self.is_tame.load(Ordering::Relaxed));
        nbt.put_bool("Saddled", self.is_saddled.load(Ordering::Relaxed));
        nbt.put_int("DashCooldown", self.dash_cooldown.load(Ordering::Relaxed));
        if let Some(owner) = self.owner.load() {
            nbt.put_uuid("Owner", owner);
        }
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if !self.is_zombie() {
            self.read_ageable_nbt(nbt);
            self.read_animal_nbt(nbt);
        }
        if let Some(is_tame) = nbt.get_bool("IsTame") {
            self.is_tame.store(is_tame, Ordering::Relaxed);
        }
        if let Some(saddled) = nbt.get_bool("Saddled") {
            self.is_saddled.store(saddled, Ordering::Relaxed);
        }
        if let Some(dash) = nbt.get_int("DashCooldown") {
            self.dash_cooldown.store(dash, Ordering::Relaxed);
        }
        if let Some(owner) = nbt.get_uuid("Owner") {
            self.owner.store(Some(owner));
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        self.mob_entity.living_entity.entity.set_synced_data(
            pumpkin_data::tracked_data::nautilus::DASH,
            self.is_dashing(),
        );
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        if !self.is_zombie() {
            self.ageable_ai_step();
        }
        let entity = &self.mob_entity.living_entity.entity;

        let Ok(passengers) = entity.passengers.try_lock() else {
            return;
        };
        if let Some(passenger) = passengers.first()
            && let Some(player) = passenger.cast_any().downcast_ref::<Player>()
        {
            let world = entity.world.load();
            let game_time = world.get_world_age();
            if game_time % 40 == 0 {
                let player_arc = world.get_player_by_uuid(player.gameprofile.id);
                if let Some(p) = player_arc {
                    p.add_effect(Effect {
                        effect_type: &StatusEffect::BREATH_OF_THE_NAUTILUS,
                        duration: 60,
                        amplifier: 0,
                        ambient: true,
                        show_particles: true,
                        show_icon: true,
                        blend: true,
                    });
                }
            }
        }

        if self.is_dashing() && self.dash_cooldown.load(Ordering::Relaxed) < 35 {
            self.set_dashing(false);
        }

        let cooldown = self.dash_cooldown.load(Ordering::Relaxed);
        if cooldown > 0 {
            let next = cooldown - 1;
            self.dash_cooldown.store(next, Ordering::Relaxed);
            if next == 0 {
                let world = entity.world.load();
                world.play_sound(
                    self.get_dash_ready_sound(),
                    SoundCategory::Neutral,
                    &entity.pos.load(),
                );
            }
        }

        if entity.touching_water.load(Ordering::Relaxed) {
            let velo = entity.velocity.load();
            let speed = velo.length();
            let prob = (speed * 2.0).clamp(0.15, 1.0);
            if rand::random::<f64>() < prob {
                let world = entity.world.load();
                let pos = entity.pos.load();
                world.spawn_particle(
                    pos + Vector3::new(0.0, 0.25, 0.0),
                    Vector3::new(0.4, 0.4, 0.4),
                    0.5,
                    2,
                    Particle::Bubble,
                );
            }
        }
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let mob_entity = &self.mob_entity;
        let entity = &mob_entity.living_entity.entity;

        if !self.is_tame()
            && item_stack
                .item
                .has_tag(&tag::Item::MINECRAFT_NAUTILUS_TAMING_ITEMS)
        {
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            if rand::random::<u32>().is_multiple_of(3) {
                self.set_tame(true, Some(player.gameprofile.id));
                let world = entity.world.load();
                world.send_entity_status(entity, EntityStatus::TamingSucceeded, None);
            } else {
                let world = entity.world.load();
                world.send_entity_status(entity, EntityStatus::TamingFailed, None);
            }
            let world = entity.world.load();
            world.play_sound(
                self.get_eat_sound(),
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
            return true;
        }

        if self.is_tame() && !player.get_entity().is_sneaking() {
            if !self.is_saddled.load(Ordering::Relaxed)
                && item_stack.item == &pumpkin_data::item::Item::SADDLE
            {
                item_stack.decrement_unless_creative(player.gamemode.load(), 1);
                self.is_saddled.store(true, Ordering::Relaxed);
                let world = entity.world.load();
                world.play_sound(
                    Sound::ItemNautilusSaddleEquip,
                    SoundCategory::Neutral,
                    &entity.pos.load(),
                );
                return true;
            }

            let world = player.world();
            if let Some(vehicle) = world.get_entity_by_id(entity.entity_id)
                && let Some(passenger) = world.get_player_by_id(player.entity_id())
            {
                entity.add_passenger(vehicle, passenger as Arc<dyn EntityBase>);
                return true;
            }
        }

        if self.is_zombie()
            && self.is_food(item_stack)
            && self.mob_entity.living_entity.health.load()
                < self.mob_entity.living_entity.get_max_health()
        {
            self.mob_entity.living_entity.heal(2.0);
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            return true;
        }
        !self.is_zombie() && self.animal_interact(player, item_stack, self.get_ambient_sound())
    }

    fn is_saddled(&self) -> bool {
        self.is_saddled.load(Ordering::Relaxed)
    }

    fn can_be_saddled(&self) -> bool {
        self.mob_entity.living_entity.entity.is_alive()
    }

    fn set_saddled(&self, saddled: bool) {
        self.is_saddled.store(saddled, Ordering::Relaxed);
    }
}
