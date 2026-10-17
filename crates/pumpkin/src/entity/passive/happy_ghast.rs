use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crate::entity::ai::{
    goal::{
        Controls, Goal, escape_danger::EscapeDangerGoal,
        water_avoiding_random_flying::WaterAvoidingRandomFlyingGoal,
    },
    pathfinder::{Navigator, NavigatorGoal},
};
use crossbeam::atomic::AtomicCell;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{
    attributes::Attributes, data_component_impl::EquipmentSlot,
    environment_attribute::EnvironmentAttribute,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::java::server::play::SPlayerInput;
use pumpkin_util::math::{position::BlockPos, vector3::Vector3, wrap_degrees};

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal, tempt::TemptGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

pub const HAPPY_GHAST_FOOD: &[&Item] = &[&Item::SNOWBALL];

/// Represents a Happy Ghast, a passive flying mob.
///
/// Wiki: <https://minecraft.wiki/w/Happy_Ghast>
pub struct HappyGhastEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub server_still_timeout: AtomicI32,
    pub leash_holder_time: AtomicI32,
    pub is_leash_holder: AtomicBool,
    pub stays_still: AtomicBool,
    home: AtomicCell<BlockPos>,
    ridden_last_tick: AtomicBool,
}

impl HappyGhastEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let home = entity.block_pos.load();
        let mob_entity = MobEntity::new(entity);
        *mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Navigator::flying();
        let happy_ghast = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            server_still_timeout: AtomicI32::new(0),
            leash_holder_time: AtomicI32::new(0),
            is_leash_holder: AtomicBool::new(false),
            stays_still: AtomicBool::new(false),
            home: AtomicCell::new(home),
            ridden_last_tick: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(happy_ghast);
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

            goal_selector.add_goal(0, Box::new(HappyGhastRideGoal));
            goal_selector.add_goal(1, EscapeDangerGoal::new(2.0));
            goal_selector.add_goal(
                2,
                Box::new(TemptGoal::with_tag(
                    1.25,
                    &tag::Item::MINECRAFT_HAPPY_GHAST_TEMPT_ITEMS,
                    3.5,
                )),
            );
            goal_selector.add_goal(3, Box::new(GhastlingFollowGoal::default()));
            goal_selector.add_goal(5, Box::new(WaterAvoidingRandomFlyingGoal::new(1.0)));
            goal_selector.add_goal(
                3,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(4, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }

    fn has_harness(&self) -> bool {
        !self
            .mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::BODY)
            .is_empty()
    }

    pub fn set_server_still_timeout(&self, timeout: i32) {
        self.server_still_timeout.store(timeout, Ordering::Relaxed);
        self.sync_stay_still_flag();
    }

    #[must_use]
    pub fn is_on_still_timeout(&self) -> bool {
        self.stays_still.load(Ordering::Relaxed)
            || self.server_still_timeout.load(Ordering::Relaxed) > 0
    }

    fn set_leash_holder(&self, is_leash_holder: bool) {
        self.is_leash_holder
            .store(is_leash_holder, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::happy_ghast::IS_LEASH_HOLDER,
            is_leash_holder,
        );
    }

    fn sync_stay_still_flag(&self) {
        let stays_still = self.server_still_timeout.load(Ordering::Relaxed) > 0;
        self.stays_still.store(stays_still, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::happy_ghast::STAYS_STILL,
            stays_still,
        );
    }
}

impl AgeableMob for HappyGhastEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for HappyGhastEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack
            .item
            .has_tag(&tag::Item::MINECRAFT_HAPPY_GHAST_FOOD)
            || HAPPY_GHAST_FOOD.iter().any(|i| i.id == item_stack.item.id)
    }
}

impl Mob for HappyGhastEntity {
    fn get_mob_gravity(&self) -> f64 {
        0.0
    }
    fn get_mob_y_velocity_drag(&self) -> Option<f64> {
        Some(0.91)
    }
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let has_passengers = entity.has_passengers();
        if self
            .ridden_last_tick
            .swap(has_passengers, Ordering::Relaxed)
            && !has_passengers
        {
            self.set_server_still_timeout(10);
            self.home.store(entity.block_pos.load());
            entity.play_sound(Sound::EntityHappyGhastHarnessGogglesUp);
        }
        let bounds = entity.bounding_box.load();
        if !self.is_baby()
            && world.players.load().iter().any(|p| {
                let pos = p.position();
                !p.is_spectator()
                    && !p.get_entity().has_vehicle()
                    && pos.y >= bounds.max.y - 0.01
                    && pos.y <= bounds.max.y + f64::from(entity.height()) * 0.5
                    && pos.x >= bounds.min.x - 1.0
                    && pos.x <= bounds.max.x + 1.0
                    && pos.z >= bounds.min.z - 1.0
                    && pos.z <= bounds.max.z + 1.0
            })
        {
            self.set_server_still_timeout(10);
        }
        if !has_passengers && !entity.is_leashed() {
            let radius = if self.is_baby() || self.has_harness() {
                32.0
            } else {
                64.0
            };
            let home = self.home.load().to_centered_f64();
            let distance = entity.pos.load().squared_distance_to_vec(&home);
            if distance > (radius + 16.0) * (radius + 16.0) {
                self.home.store(entity.block_pos.load());
            } else if distance > radius * radius && world.get_world_age() % 20 == 0 {
                self.mob_entity
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set_progress(NavigatorGoal::new(entity.pos.load(), home, 1.0));
            }
        }
        if self.is_on_still_timeout() {
            entity.set_velocity(Vector3::new(0.0, 0.0, 0.0));
            self.mob_entity
                .living_entity
                .movement_input
                .store(Vector3::new(0.0, 0.0, 0.0));
        }
    }

    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        nbt.put_int(
            "still_timeout",
            self.server_still_timeout.load(Ordering::Relaxed),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        if let Some(timeout) = nbt.get_int("still_timeout") {
            self.set_server_still_timeout(timeout);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ageable_ai_step();

        let leash_time = self.leash_holder_time.load(Ordering::Relaxed);
        if leash_time > 0 {
            self.leash_holder_time.fetch_sub(1, Ordering::Relaxed);
        }
        self.set_leash_holder(leash_time > 0);

        let still_timeout = self.server_still_timeout.load(Ordering::Relaxed);
        if still_timeout > 0 {
            let entity = self.get_entity();
            if entity.age.load(Ordering::Relaxed) > 60 {
                self.server_still_timeout.fetch_sub(1, Ordering::Relaxed);
            }
            self.sync_stay_still_flag();
        }

        // Continuous healing
        let entity = self.get_entity();
        if self.mob_entity.living_entity.is_alive() {
            let living = &self.mob_entity.living_entity;
            let current_health = living.health.load();
            let max_health = living.get_max_health();
            if current_health < max_health {
                let world = entity.world.load();
                let ticks = world.get_world_age();
                let cloud = world.environment_attributes().get_value_f32(
                    EnvironmentAttribute::VisualCloudHeight,
                    &entity.block_pos.load(),
                );
                let heal_interval = if entity.pos.load().y >= f64::from(cloud) - 4.0
                    || world.is_raining_at(&entity.block_pos.load())
                {
                    20
                } else {
                    600
                };
                if ticks % heal_interval == 0 {
                    living.heal(1.0);
                }
            }
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::happy_ghast::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::happy_ghast::IS_LEASH_HOLDER,
            self.is_leash_holder.load(Ordering::Relaxed),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::happy_ghast::STAYS_STILL,
            self.stays_still.load(Ordering::Relaxed),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if self.is_baby() {
            return self.animal_interact(player, item_stack, Sound::EntityGhastlingAmbient);
        }
        if item_stack.item.has_tag(&tag::Item::MINECRAFT_HARNESSES) && !self.has_harness() {
            let mut harness = item_stack.clone();
            harness.item_count = 1;
            self.mob_entity
                .living_entity
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .put(&EquipmentSlot::BODY, harness.clone());
            self.mob_entity
                .living_entity
                .send_equipment_changes(&[(EquipmentSlot::BODY, harness)]);
            self.mob_entity
                .living_entity
                .equipment_drop_chances
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(EquipmentSlot::BODY, 2.0);
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            return true;
        }
        if self.has_harness() && !player.get_entity().is_sneaking() {
            let entity = self.get_entity();
            let world = entity.world.load();
            if entity
                .passengers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len()
                < 4
                && let Some(vehicle) = world.get_entity_by_id(entity.entity_id)
            {
                let first = !entity.has_passengers();
                entity.add_passenger(vehicle, player.clone() as Arc<dyn EntityBase>);
                self.set_server_still_timeout(0);
                if first {
                    entity.play_sound(Sound::EntityHappyGhastHarnessGogglesDown);
                }
                return true;
            }
        }
        self.mob_entity.mob_interact(player, item_stack)
    }
}

struct HappyGhastRideGoal;
impl Goal for HappyGhastRideGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.cast_any()
            .downcast_ref::<HappyGhastEntity>()
            .is_some_and(|g| g.is_on_still_timeout() || g.get_entity().has_passengers())
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob)
    }
    fn start(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
    }
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(ghast) = mob.cast_any().downcast_ref::<HappyGhastEntity>() else {
            return;
        };
        let entity = mob.get_entity();
        mob.get_mob_entity()
            .living_entity
            .movement_input
            .store(Vector3::new(0.0, 0.0, 0.0));
        if ghast.is_on_still_timeout() || !ghast.has_harness() {
            return;
        }
        let passenger = entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .cloned();
        let Some(passenger) = passenger else {
            return;
        };
        let Some(player) = passenger.get_player() else {
            return;
        };
        let flags = player.last_input.load(Ordering::Relaxed);
        let strafe = f64::from(
            (flags & SPlayerInput::LEFT != 0) as i32 - (flags & SPlayerInput::RIGHT != 0) as i32,
        );
        let direction = if flags & SPlayerInput::FORWARD != 0 {
            1.0
        } else if flags & SPlayerInput::BACKWARD != 0 {
            -0.5
        } else {
            0.0
        };
        let pitch = f64::from(player.get_entity().pitch.load().to_radians());
        let up = -pitch.sin() * direction
            + if flags & SPlayerInput::JUMP != 0 {
                0.5
            } else {
                0.0
            };
        let forward = pitch.cos() * direction;
        let yaw = entity.yaw.load()
            + wrap_degrees(player.get_entity().yaw.load() - entity.yaw.load()) * 0.08;
        entity.set_rotation(yaw, player.get_entity().pitch.load() * 0.5);
        entity.head_yaw.store(yaw);
        entity.body_yaw.store(yaw);
        let yaw = f64::from(yaw.to_radians());
        let speed = mob
            .get_mob_entity()
            .living_entity
            .get_attribute_value(&Attributes::FLYING_SPEED)
            * 3.9;
        let input = Vector3::new(
            strafe * yaw.cos() - forward * yaw.sin(),
            up,
            forward * yaw.cos() + strafe * yaw.sin(),
        ) * speed;
        entity.set_velocity(entity.velocity.load() * 0.9 + input * 0.1);
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK | Controls::JUMP
    }
}
#[derive(Default)]
struct GhastlingFollowGoal {
    target: Option<Arc<dyn EntityBase>>,
    timer: i32,
}
impl Goal for GhastlingFollowGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(ghast) = mob.cast_any().downcast_ref::<HappyGhastEntity>() else {
            return false;
        };
        if !ghast.is_baby() {
            return false;
        }
        let entity = mob.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();
        self.target = world
            .get_nearest_player(pos, 16.0, |p| !p.is_spectator())
            .map(|p| p as Arc<dyn EntityBase>)
            .or_else(|| {
                world.get_nearest_entity(pos, 16.0, Some(&[&EntityType::HAPPY_GHAST]), |other| {
                    other.get_entity().entity_id != entity.entity_id
                        && other.get_entity().age.load(Ordering::Relaxed) >= 0
                })
            });
        self.target
            .as_ref()
            .is_some_and(|t| pos.squared_distance_to_vec(&t.get_entity().pos.load()) > 9.0)
    }
    fn start(&mut self, _mob: &dyn Mob) {
        self.timer = 0;
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.as_ageable().is_some_and(AgeableMob::is_baby)
            && self.target.as_ref().is_some_and(|t| {
                let d = t
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&mob.get_entity().pos.load());
                t.get_entity().is_alive() && (9.0..=256.0).contains(&d)
            })
    }
    fn tick(&mut self, mob: &dyn Mob) {
        self.timer += 1;
        if self.timer % 10 == 1
            && let Some(t) = &self.target
        {
            mob.get_mob_entity()
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_progress(NavigatorGoal::new(
                    mob.get_entity().pos.load(),
                    t.get_entity().pos.load(),
                    1.1,
                ));
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
