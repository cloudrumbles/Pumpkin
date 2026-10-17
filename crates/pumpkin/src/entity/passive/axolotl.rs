use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crate::entity::ai::goal::{
    Controls, Goal,
    aquatic::{self, RandomSwimGoal},
};
use crate::entity::ai::{
    brain::{
        Brain,
        memory::{PackedMemories, types},
    },
    pathfinder::Navigator,
};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{damage::DamageType, effect::StatusEffect, potion::Effect};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        active_target::ActiveTargetGoal, breed::BreedGoal, escape_danger::EscapeDangerGoal,
        follow_parent::FollowParentGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal, revenge::RevengeGoal,
        tempt::TemptGoal, try_find_water::TryFindWaterGoal, wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

const TEMPT_ITEMS: &[&Item] = &[&Item::TROPICAL_FISH_BUCKET];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum AxolotlVariant {
    #[default]
    Lucy = 0,
    Wild = 1,
    Gold = 2,
    Cyan = 3,
    Blue = 4,
}

impl AxolotlVariant {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::Wild,
            2 => Self::Gold,
            3 => Self::Cyan,
            4 => Self::Blue,
            _ => Self::Lucy,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub fn random_variant() -> Self {
        let mut rng = rand::rng();
        if rng.random_range(0..1200) == 0 {
            Self::Blue
        } else {
            match rng.random_range(0..4) {
                1 => Self::Wild,
                2 => Self::Gold,
                3 => Self::Cyan,
                _ => Self::Lucy,
            }
        }
    }
}

pub struct AxolotlEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub variant: AtomicI32,
    pub playing_dead: AtomicBool,
    pub from_bucket: AtomicBool,
    pub play_dead_ticks: AtomicI32,
    air_supply: AtomicI32,
    was_hunting: AtomicBool,
}

impl AxolotlEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        *mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Navigator::amphibious(false);
        let variant = AxolotlVariant::from_id(rand::random_range(0..4));
        let axolotl = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(variant.id()),
            playing_dead: AtomicBool::new(false),
            from_bucket: AtomicBool::new(false),
            play_dead_ticks: AtomicI32::new(0),
            air_supply: AtomicI32::new(6000),
            was_hunting: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(axolotl);
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

            goal_selector.add_goal(0, Box::new(PlayDeadGoal));
            goal_selector.add_goal(1, EscapeDangerGoal::new(1.0));
            goal_selector.add_goal(2, BreedGoal::new(0.2));
            goal_selector.add_goal(3, Box::new(TemptGoal::new(0.5, TEMPT_ITEMS, false)));
            goal_selector.add_goal(4, Box::new(FollowParentGoal::new(0.6)));
            goal_selector.add_goal(5, Box::new(MeleeAttackGoal::new(0.6, false)));
            goal_selector.add_goal(6, Box::new(TryFindWaterGoal));
            goal_selector.add_goal(7, Box::new(RandomSwimGoal::new(0.5, 40)));
            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new(0.15)));
            goal_selector.add_goal(
                9,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(10, Box::new(RandomLookAroundGoal::default()));
        };

        {
            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            target_selector.add_goal(1, Box::new(RevengeGoal::new(true)));
            let weak = Arc::downgrade(&mob_arc);
            target_selector.add_goal(
                2,
                ActiveTargetGoal::predicated(
                    &mob_arc.mob_entity,
                    10,
                    true,
                    move |target, _world| {
                        weak.upgrade().is_some_and(|axolotl| {
                            !axolotl.is_playing_dead()
                                && !axolotl.mob_entity.is_in_love()
                                && target.entity.touching_water.load(Ordering::Relaxed)
                                && (target
                                    .entity
                                    .entity_type
                                    .has_tag(&tag::EntityType::MINECRAFT_AXOLOTL_ALWAYS_HOSTILES)
                                    || (target
                                        .entity
                                        .entity_type
                                        .has_tag(&tag::EntityType::MINECRAFT_AXOLOTL_HUNT_TARGETS)
                                        && !axolotl
                                            .mob_entity
                                            .brain
                                            .lock()
                                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                                            .has_memory_value(types::HAS_HUNTING_COOLDOWN.id())))
                        })
                    },
                ),
            );
        };

        mob_arc
    }

    #[must_use]
    pub fn get_variant(&self) -> AxolotlVariant {
        AxolotlVariant::from_id(self.variant.load(Ordering::Relaxed))
    }

    pub fn set_variant(&self, variant: AxolotlVariant) {
        self.variant.store(variant.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_VARIANT,
            VarInt(variant.id()),
        );
    }

    #[must_use]
    pub fn is_playing_dead(&self) -> bool {
        self.playing_dead.load(Ordering::Relaxed)
    }

    pub fn set_playing_dead(&self, playing_dead: bool) {
        self.playing_dead.store(playing_dead, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_PLAYING_DEAD,
            playing_dead,
        );
    }

    #[must_use]
    pub fn is_from_bucket(&self) -> bool {
        self.from_bucket.load(Ordering::Relaxed)
    }

    pub fn set_from_bucket(&self, from_bucket: bool) {
        self.from_bucket.store(from_bucket, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::FROM_BUCKET,
            from_bucket,
        );
    }
}

impl AgeableMob for AxolotlEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for AxolotlEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_AXOLOTL_FOOD)
    }
}

impl Mob for AxolotlEntity {
    fn requires_custom_persistence(&self) -> bool {
        self.is_from_bucket()
    }
    fn get_mob_gravity(&self) -> f64 {
        aquatic::aquatic_gravity(&self.mob_entity)
    }
    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        let mut brain = Brain::default();
        brain.register_memory(types::PLAY_DEAD_TICKS.id());
        brain.register_memory(types::HAS_HUNTING_COOLDOWN.id());
        brain.load_packed(packed);
        brain
    }
    fn on_damage(&self, _damage: DamageType, source: Option<&dyn EntityBase>) {
        let living = &self.mob_entity.living_entity;
        let damage = living.last_damage_taken.load();
        if !self.mob_entity.is_no_ai()
            && source.is_some()
            && living.is_alive()
            && self.get_entity().touching_water.load(Ordering::Relaxed)
            && !self.is_playing_dead()
            && rand::random_range(0..3) == 0
            && ((rand::random_range(0..3) as f32) < damage
                || (living.health.load() + damage) / living.get_max_health() < 0.5)
        {
            self.mob_entity
                .brain
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set(types::PLAY_DEAD_TICKS, 200);
            self.set_playing_dead(true);
        }
    }
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        {
            let mut brain = self
                .mob_entity
                .brain
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            brain.tick(&world, self, world.get_world_age());
            if let Some(ticks) = brain.get(types::PLAY_DEAD_TICKS).copied() {
                if ticks <= 1
                    || !entity.touching_water.load(Ordering::Relaxed)
                    || self.mob_entity.is_in_love()
                {
                    brain.erase(types::PLAY_DEAD_TICKS.id());
                    self.set_playing_dead(false);
                } else {
                    brain.set(types::PLAY_DEAD_TICKS, ticks - 1);
                    self.set_playing_dead(true);
                }
            }
        }
        if self.is_playing_dead() {
            self.mob_entity.set_target(None);
            self.mob_entity
                .living_entity
                .movement_input
                .store(Vector3::new(0.0, 0.0, 0.0));
        } else if let Some(target) = self.mob_entity.get_target()
            && (!target.get_living_entity().is_some_and(|l| l.is_alive())
                || !target.get_entity().touching_water.load(Ordering::Relaxed)
                || self.mob_entity.is_in_love())
        {
            self.mob_entity.set_target(None);
        }
        let hunting = self.mob_entity.get_target().is_some();
        if self.was_hunting.swap(hunting, Ordering::Relaxed) && !hunting {
            self.mob_entity
                .brain
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .set_with_expiry(types::HAS_HUNTING_COOLDOWN, true, 2400);
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
        nbt.put_short("Air", self.air_supply.load(Ordering::Relaxed) as i16);
        nbt.put_int("Variant", self.get_variant().id());
        nbt.put_bool("FromBucket", self.is_from_bucket());
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        self.air_supply.store(
            i32::from(nbt.get_short("Air").unwrap_or(6000)),
            Ordering::Relaxed,
        );
        if let Some(variant) = nbt.get_int("Variant") {
            self.set_variant(AxolotlVariant::from_id(variant));
        }
        if let Some(from_bucket) = nbt.get_bool("FromBucket") {
            self.set_from_bucket(from_bucket);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick(&self, caller: &dyn EntityBase) {
        self.ageable_ai_step();
        if self.mob_entity.is_no_ai() {
            return;
        }
        let entity = self.get_entity();
        if entity.touching_water.load(Ordering::Relaxed)
            || entity.world.load().is_raining_at(&entity.block_pos.load())
        {
            self.air_supply.store(6000, Ordering::Relaxed);
        } else if self.air_supply.fetch_sub(1, Ordering::Relaxed) <= -19 {
            self.air_supply.store(0, Ordering::Relaxed);
            caller.damage(caller, 2.0, DamageType::DRY_OUT);
        }
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::axolotl::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_VARIANT,
            VarInt(self.get_variant().id()),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::DATA_PLAYING_DEAD,
            self.is_playing_dead(),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::axolotl::FROM_BUCKET,
            self.is_from_bucket(),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if crate::item::items::bucket::try_capture_mob(
            player,
            item_stack,
            self,
            &Item::AXOLOTL_BUCKET,
            Sound::ItemBucketFillAxolotl,
        ) {
            return true;
        }
        let fish_bucket = item_stack.item == &Item::TROPICAL_FISH_BUCKET;
        let result = self.animal_interact(player, item_stack, Sound::EntityAxolotlIdleAir);
        if result && fish_bucket && player.gamemode.load() != pumpkin_util::GameMode::Creative {
            *item_stack = ItemStack::new(1, &Item::WATER_BUCKET);
        }
        result
    }
}

struct PlayDeadGoal;
impl Goal for PlayDeadGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.cast_any()
            .downcast_ref::<AxolotlEntity>()
            .is_some_and(AxolotlEntity::is_playing_dead)
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
        mob.get_mob_entity().set_target(None);
        mob.get_mob_entity().living_entity.add_effect(Effect {
            effect_type: &StatusEffect::REGENERATION,
            duration: 200,
            amplifier: 0,
            ambient: false,
            show_particles: true,
            show_icon: true,
            blend: false,
        });
    }
    fn tick(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity()
            .living_entity
            .movement_input
            .store(Vector3::new(0.0, 0.0, 0.0));
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK | Controls::JUMP
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
}
