use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

use pumpkin_data::entity::{EntityPose, EntityStatus, EntityType};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::bedrock::server::actor_event::ActorEventID;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        ConditionalGoal, Controls, Goal, active_target::ActiveTargetGoal,
        avoid_entity::AvoidEntityGoal, breed::BreedGoal, leap_at_target::LeapAtTargetGoal,
        look_at_entity::LookAtEntityGoal, ocelot_attack::OcelotAttackGoal, swim::SwimGoal,
        tempt::TemptGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

/// Represents an Ocelot, a shy passive mob found in jungles.
///
/// Wiki: <https://minecraft.wiki/w/Ocelot>
pub struct OcelotEntity {
    pub mob_entity: MobEntity,
    pub is_trusting: AtomicBool,
    ageable_data: AgeableData,
    is_tempted: AtomicBool,
}

impl OcelotEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let ocelot = Self {
            mob_entity,
            is_trusting: AtomicBool::new(false),
            ageable_data: AgeableData::default(),
            is_tempted: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(ocelot);
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

            // Goal 1: FloatGoal (SwimGoal)
            goal_selector.add_goal(1, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                3,
                Box::new(OcelotTemptGoal(TemptGoal::with_tag(
                    0.6,
                    &tag::Item::MINECRAFT_OCELOT_FOOD,
                    2.5,
                ))),
            );
            // Goal 4: OcelotAvoidEntityGoal (when not trusting)
            goal_selector.add_goal(
                4,
                ConditionalGoal::new(
                    Box::new(AvoidEntityGoal::new(&EntityType::PLAYER, 16.0, 0.8, 1.33)),
                    |mob| {
                        mob.cast_any()
                            .downcast_ref::<Self>()
                            .is_some_and(|ocelot| !ocelot.is_trusting())
                    },
                ),
            );
            goal_selector.add_goal(7, Box::new(LeapAtTargetGoal::new(0.3)));
            goal_selector.add_goal(8, Box::new(OcelotAttackGoal::new()));
            // Goal 9: BreedGoal
            goal_selector.add_goal(9, BreedGoal::new(0.8));
            // Goal 10: WaterAvoidingRandomStrollGoal (WanderAroundGoal)
            goal_selector.add_goal(
                10,
                Box::new(WanderAroundGoal::avoiding_water(0.8, 0.000_010_000_001)),
            );
            // Goal 11: LookAtPlayerGoal
            goal_selector.add_goal(
                11,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 10.0),
            );
        };

        {
            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            // Target Goal 1: NearestAttackableTargetGoal for Chicken and Turtle
            target_selector.add_goal(
                1,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::CHICKEN, false),
            );
            target_selector.add_goal(
                1,
                Box::new(ActiveTargetGoal::new(
                    &mob_arc.mob_entity,
                    &EntityType::TURTLE,
                    10,
                    false,
                    false,
                    Some(
                        |target: &crate::entity::living::LivingEntity, _: &crate::world::World| {
                            target.entity.age.load(Ordering::Relaxed) < 0
                                && !target.entity.touching_water.load(Ordering::Relaxed)
                        },
                    ),
                )),
            );
        };

        mob_arc
    }

    pub fn is_trusting(&self) -> bool {
        self.is_trusting.load(Ordering::Relaxed)
    }

    pub fn set_trusting(&self, trusting: bool) {
        self.is_trusting.store(trusting, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::ocelot::TRUSTING, trusting);
    }
}

impl Animal for OcelotEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        let item = item_stack.get_item();
        item.has_tag(&tag::Item::MINECRAFT_OCELOT_FOOD)
    }
}

impl AgeableMob for OcelotEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Mob for OcelotEntity {
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ageable_ai_step();
    }

    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        !self.is_trusting() && self.get_entity().tick_count.load(Ordering::Relaxed) > 2400
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let speed = {
            let nav = self
                .mob_entity
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if nav.is_idle() {
                0.0
            } else {
                nav.get_speed_modifier()
            }
        };
        self.get_entity().set_pose(if speed == 0.6 {
            EntityPose::Crouching
        } else {
            EntityPose::Standing
        });
        self.get_entity().set_sprinting(speed == 1.33);
    }
    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("Trusting", self.is_trusting.load(Ordering::Relaxed));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        if let Some(trusting) = nbt.get_bool("Trusting") {
            self.is_trusting.store(trusting, Ordering::Relaxed);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::ocelot::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::ocelot::TRUSTING,
            self.is_trusting.load(Ordering::Relaxed),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        let is_food = self.is_food(item_stack);
        let dist_sqr = self
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&player.get_entity().pos.load());

        if self.is_tempted.load(Ordering::Relaxed)
            && !self.is_trusting()
            && is_food
            && dist_sqr < 9.0
        {
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);

            let mut rng = rand::rng();
            if rng.random_range(0..3) == 0 {
                self.set_trusting(true);
                self.get_entity().world.load().send_entity_status(
                    self.get_entity(),
                    EntityStatus::TrustingSucceeded,
                    Some(ActorEventID::TamingSucceeded),
                );
            } else {
                self.get_entity().world.load().send_entity_status(
                    self.get_entity(),
                    EntityStatus::TrustingFailed,
                    Some(ActorEventID::TamingFailed),
                );
            }

            return true;
        }

        self.animal_interact(
            player,
            item_stack,
            pumpkin_data::sound::Sound::EntityOcelotAmbient,
        )
    }
}

struct OcelotTemptGoal(TemptGoal);

impl Goal for OcelotTemptGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.0.can_start(mob)
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.0.set_can_scare(
            mob.cast_any()
                .downcast_ref::<OcelotEntity>()
                .is_some_and(|ocelot| !ocelot.is_trusting()),
        );
        self.0.should_continue(mob)
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.0.start(mob);
        if let Some(ocelot) = mob.cast_any().downcast_ref::<OcelotEntity>() {
            ocelot.is_tempted.store(true, Ordering::Relaxed);
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        self.0.stop(mob);
        if let Some(ocelot) = mob.cast_any().downcast_ref::<OcelotEntity>() {
            ocelot.is_tempted.store(false, Ordering::Relaxed);
        }
    }
    fn tick(&mut self, mob: &dyn Mob) {
        self.0.tick(mob);
    }
    fn should_run_every_tick(&self) -> bool {
        self.0.should_run_every_tick()
    }
    fn controls(&self) -> Controls {
        self.0.controls()
    }
}
