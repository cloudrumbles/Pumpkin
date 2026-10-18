use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag;
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        ConditionalGoal, Controls, Goal, active_target::ActiveTargetGoal,
        escape_danger::EscapeDangerGoal, follow_parent::FollowParentGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        melee_attack::MeleeAttackGoal, revenge::RevengeGoal, swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

pub struct PolarBearEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub standing: AtomicBool,
    warning_sound_ticks: AtomicI32,
}

impl PolarBearEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let polar_bear = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            standing: AtomicBool::new(false),
            warning_sound_ticks: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(polar_bear);
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

            goal_selector.add_goal(0, Box::new(SwimGoal::default()));
            goal_selector.add_goal(
                1,
                Box::new(PolarBearMeleeGoal(
                    MeleeAttackGoal::new(1.25, true).with_attack_handler(Self::attack_tick),
                )),
            );
            goal_selector.add_goal(
                1,
                EscapeDangerGoal::with_tag_selector(2.0, |mob| {
                    if mob.as_ageable().is_some_and(AgeableMob::is_baby) {
                        &tag::DamageType::MINECRAFT_PANIC_CAUSES
                    } else {
                        &tag::DamageType::MINECRAFT_PANIC_ENVIRONMENTAL_CAUSES
                    }
                }),
            );
            goal_selector.add_goal(4, Box::new(FollowParentGoal::new(1.25)));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                6,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(7, Box::new(RandomLookAroundGoal::default()));
        };

        {
            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            target_selector.add_goal(
                1,
                Box::new(PolarBearRevengeGoal(
                    RevengeGoal::new(true).alerting_others_when(|mob, other, _| {
                        mob.as_ageable().is_some_and(AgeableMob::is_baby)
                            && other.as_ageable().is_some_and(|bear| !bear.is_baby())
                    }),
                )),
            );
            let bear = Arc::downgrade(&mob_arc);
            target_selector.add_goal(
                2,
                Box::new(
                    ActiveTargetGoal::new(
                        &mob_arc.mob_entity,
                        &EntityType::PLAYER,
                        20,
                        true,
                        true,
                        Some(
                            move |_: &crate::entity::living::LivingEntity,
                                  world: &crate::world::World| {
                                let Some(bear) = bear.upgrade() else {
                                    return false;
                                };
                                if bear.is_baby() {
                                    return false;
                                }
                                let area =
                                    bear.get_entity().bounding_box.load().expand(8.0, 4.0, 8.0);
                                world.get_entities_at_box(&area).iter().any(|entity| {
                                    entity.get_entity().entity_type == &EntityType::POLAR_BEAR
                                        && entity
                                            .get_mob()
                                            .and_then(Mob::as_ageable)
                                            .is_some_and(AgeableMob::is_baby)
                                })
                            },
                        ),
                    )
                    .with_range_multiplier(0.5),
                ),
            );
            target_selector.add_goal(
                4,
                ConditionalGoal::new(
                    ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::FOX, true),
                    |mob| mob.as_ageable().is_some_and(|bear| !bear.is_baby()),
                ),
            );
        };

        mob_arc
    }

    #[must_use]
    pub fn is_standing(&self) -> bool {
        self.standing.load(Ordering::Relaxed)
    }

    pub fn set_standing(&self, standing: bool) {
        self.standing.store(standing, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::polar_bear::DATA_STANDING_ID,
            standing,
        );
    }

    fn attack_tick(mob: &dyn Mob, target: &dyn EntityBase, goal: &mut MeleeAttackGoal) {
        let Some(bear) = mob.cast_any().downcast_ref::<Self>() else {
            return;
        };
        if goal.can_perform_attack(mob, target) {
            goal.cooldown = goal.get_max_cooldown();
            mob.get_mob_entity().try_attack(mob, target);
            bear.set_standing(false);
        } else if mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&target.get_entity().pos.load())
            < f64::from(target.get_entity().entity_dimension.load().width + 3.0).powi(2)
        {
            if goal.cooldown <= 0 {
                bear.set_standing(false);
                goal.cooldown = goal.get_max_cooldown();
            }
            if goal.cooldown <= 10 {
                bear.set_standing(true);
                if bear.warning_sound_ticks.load(Ordering::Relaxed) <= 0 {
                    bear.get_entity().play_sound(Sound::EntityPolarBearWarning);
                    bear.warning_sound_ticks.store(40, Ordering::Relaxed);
                }
            }
        } else {
            goal.cooldown = goal.get_max_cooldown();
            bear.set_standing(false);
        }
    }
}

impl AgeableMob for PolarBearEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for PolarBearEntity {
    fn is_food(&self, _item_stack: &ItemStack) -> bool {
        false
    }
}

impl Mob for PolarBearEntity {
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ageable_ai_step();
        if self.warning_sound_ticks.load(Ordering::Relaxed) > 0 {
            self.warning_sound_ticks.fetch_sub(1, Ordering::Relaxed);
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::polar_bear::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::polar_bear::DATA_STANDING_ID,
            self.is_standing(),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack, Sound::EntityPolarBearAmbient)
    }
}

struct PolarBearMeleeGoal(MeleeAttackGoal);

impl Goal for PolarBearMeleeGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.0.can_start(mob)
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.0.should_continue(mob)
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.0.start(mob);
    }
    fn tick(&mut self, mob: &dyn Mob) {
        self.0.tick(mob);
    }
    fn stop(&mut self, mob: &dyn Mob) {
        self.0.stop(mob);
        if let Some(bear) = mob.cast_any().downcast_ref::<PolarBearEntity>() {
            bear.set_standing(false);
        }
    }
    fn controls(&self) -> Controls {
        self.0.controls()
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
}

struct PolarBearRevengeGoal(RevengeGoal);

impl Goal for PolarBearRevengeGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        self.0.can_start(mob)
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        !mob.as_ageable().is_some_and(AgeableMob::is_baby) && self.0.should_continue(mob)
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.0.start(mob);
        if mob.as_ageable().is_some_and(AgeableMob::is_baby) {
            mob.set_mob_target(None);
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        self.0.stop(mob);
    }
    fn controls(&self) -> Controls {
        self.0.controls()
    }
}
