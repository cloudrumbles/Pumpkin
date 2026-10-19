use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

use crate::world::World;
use pumpkin_data::attributes::Attributes;
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        ConditionalGoal, active_target::ActiveTargetGoal, avoid_entity::AvoidEntityGoal,
        breed::BreedGoal, climb_powder_snow::ClimbPowderSnowGoal, escape_danger::EscapeDangerGoal,
        look_at_entity::LookAtEntityGoal, melee_attack::MeleeAttackGoal,
        rabbit_raid::RabbitRaidGoal, revenge::RevengeGoal, swim::SwimGoal, tempt::TemptGoal,
        wander_around::WanderAroundGoal,
    },
    attributes::{Modifier, ModifierOperation},
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum RabbitVariant {
    #[default]
    Brown = 0,
    White = 1,
    Black = 2,
    WhiteSplotched = 3,
    Gold = 4,
    Salt = 5,
    Evil = 99,
}

impl RabbitVariant {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::White,
            2 => Self::Black,
            3 => Self::WhiteSplotched,
            4 => Self::Gold,
            5 => Self::Salt,
            99 => Self::Evil,
            _ => Self::Brown,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub fn random_variant(entity: &Entity) -> Self {
        let biome = entity.world.load().get_biome(&entity.block_pos.load());
        let choice = rand::random_range(0..100);
        if biome.has_tag(&tag::WorldgenBiome::MINECRAFT_SPAWNS_WHITE_RABBITS) {
            if choice < 80 {
                Self::White
            } else {
                Self::WhiteSplotched
            }
        } else if biome.has_tag(&tag::WorldgenBiome::MINECRAFT_SPAWNS_GOLD_RABBITS) {
            Self::Gold
        } else if choice < 50 {
            Self::Brown
        } else if choice < 90 {
            Self::Salt
        } else {
            Self::Black
        }
    }
}

pub struct RabbitEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub variant: AtomicI32,
    pub more_carrot_ticks: AtomicI32,
    pub(crate) jump_requested: AtomicBool,
    jump_can_control: AtomicBool,
    jump_delay: AtomicI32,
    jump_ticks: AtomicI32,
    jump_duration: AtomicI32,
    was_on_ground: AtomicBool,
    evil_goals_added: AtomicBool,
}

impl RabbitEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let variant = RabbitVariant::random_variant(&entity);
        let mob_entity = MobEntity::new(entity);
        let rabbit = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(variant.id()),
            more_carrot_ticks: AtomicI32::new(0),
            jump_requested: AtomicBool::new(false),
            jump_can_control: AtomicBool::new(false),
            jump_delay: AtomicI32::new(0),
            jump_ticks: AtomicI32::new(0),
            jump_duration: AtomicI32::new(0),
            was_on_ground: AtomicBool::new(false),
            evil_goals_added: AtomicBool::new(false),
        };
        let mob_arc = Arc::new(rabbit);
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

            goal_selector.add_goal(1, Box::new(SwimGoal::default()));
            goal_selector.add_goal(1, Box::new(ClimbPowderSnowGoal));
            goal_selector.add_goal(1, EscapeDangerGoal::new(2.2));
            goal_selector.add_goal(2, BreedGoal::new(0.8));
            goal_selector.add_goal(
                3,
                Box::new(TemptGoal::with_tag(
                    1.0,
                    &tag::Item::MINECRAFT_RABBIT_FOOD,
                    2.5,
                )),
            );
            goal_selector.add_goal(
                4,
                ConditionalGoal::new(
                    Box::new(AvoidEntityGoal::new(&EntityType::PLAYER, 8.0, 2.2, 2.2)),
                    Self::avoids_threats,
                ),
            );
            goal_selector.add_goal(
                4,
                ConditionalGoal::new(
                    Box::new(AvoidEntityGoal::new(&EntityType::WOLF, 10.0, 2.2, 2.2)),
                    Self::avoids_threats,
                ),
            );
            goal_selector.add_goal(
                4,
                ConditionalGoal::new(
                    Box::new(AvoidEntityGoal::with_filter(4.0, 2.2, 2.2, |_, target| {
                        // The Java Monster superclass excludes, for example, ghasts and slimes.
                        matches!(
                            target.get_entity().entity_type.resource_name,
                            "blaze"
                                | "bogged"
                                | "breeze"
                                | "cave_spider"
                                | "creaking"
                                | "creeper"
                                | "drowned"
                                | "elder_guardian"
                                | "enderman"
                                | "endermite"
                                | "evoker"
                                | "giant"
                                | "guardian"
                                | "husk"
                                | "illusioner"
                                | "parched"
                                | "piglin"
                                | "piglin_brute"
                                | "pillager"
                                | "ravager"
                                | "silverfish"
                                | "skeleton"
                                | "spider"
                                | "stray"
                                | "vex"
                                | "vindicator"
                                | "warden"
                                | "witch"
                                | "wither"
                                | "wither_skeleton"
                                | "zoglin"
                                | "zombie"
                                | "zombie_villager"
                                | "zombified_piglin"
                        )
                    })),
                    Self::avoids_threats,
                ),
            );
            goal_selector.add_goal(5, RabbitRaidGoal::new());
            goal_selector.add_goal(6, Box::new(WanderAroundGoal::avoiding_water(0.6, 0.001)));
            goal_selector.add_goal(
                11,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 10.0),
            );
        };

        mob_arc
    }

    #[must_use]
    pub fn get_variant(&self) -> RabbitVariant {
        RabbitVariant::from_id(self.variant.load(Ordering::Relaxed))
    }

    fn avoids_threats(mob: &dyn Mob) -> bool {
        mob.cast_any()
            .downcast_ref::<Self>()
            .is_some_and(|rabbit| rabbit.get_variant() != RabbitVariant::Evil)
    }

    fn start_jumping(&self) {
        self.mob_entity
            .living_entity
            .jumping
            .store(true, Ordering::Relaxed);
        self.jump_ticks.store(0, Ordering::Relaxed);
        self.jump_duration.store(15, Ordering::Relaxed);
        let entity = self.get_entity();
        if !entity.is_silent() {
            let pitch = ((rand::random::<f32>() - rand::random::<f32>()) * 0.2 + 1.0) * 0.8;
            entity.world.load().play_sound_fine(
                Sound::EntityRabbitJump,
                SoundCategory::Neutral,
                &entity.pos.load(),
                1.0,
                pitch,
            );
        }
    }

    fn face_point(&self, target: Vector3<f64>) {
        let pos = self.get_entity().pos.load();
        self.get_entity()
            .yaw
            .store((target.z - pos.z).atan2(target.x - pos.x).to_degrees() as f32 - 90.0);
    }

    pub fn set_variant(&self, variant: RabbitVariant) {
        self.variant.store(variant.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::rabbit::DATA_TYPE_ID,
            VarInt(variant.id()),
        );

        if variant == RabbitVariant::Evil {
            self.mob_entity
                .living_entity
                .update_attribute(&Attributes::ARMOR, |armor| {
                    armor.base_value = 8.0;
                    armor.dirty.store(true, Ordering::Relaxed);
                });
            self.mob_entity
                .living_entity
                .update_attribute(&Attributes::ATTACK_DAMAGE, |attack| {
                    attack.add_or_replace_modifier(Modifier {
                        id: "minecraft:evil".into(),
                        amount: 5.0,
                        operation: ModifierOperation::Add,
                    });
                });
            if entity.custom_name.load().is_none() {
                entity.set_custom_name(TextComponent::translate(
                    "entity.minecraft.killer_bunny",
                    Vec::new(),
                ));
            }
            if self.evil_goals_added.swap(true, Ordering::Relaxed) {
                return;
            }
            let mut goal_selector = self
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            goal_selector.add_goal(4, Box::new(MeleeAttackGoal::new(1.4, true)));

            let mut target_selector = self
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            target_selector.add_goal(1, Box::new(RevengeGoal::new(false).alerting_others()));
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&self.mob_entity, &EntityType::PLAYER, true),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&self.mob_entity, &EntityType::WOLF, true),
            );
        } else {
            self.mob_entity
                .living_entity
                .update_attribute(&Attributes::ATTACK_DAMAGE, |attack| {
                    attack.remove_modifier("minecraft:evil")
                });
        }
    }
}

impl AgeableMob for RabbitEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for RabbitEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_RABBIT_FOOD)
    }
}

impl Mob for RabbitEntity {
    fn request_jump(&self) {
        self.jump_requested.store(true, Ordering::Relaxed);
    }

    fn tick_jump_control(&self) {
        if self.jump_requested.swap(false, Ordering::Relaxed) {
            self.start_jumping();
        }
    }

    fn get_mob_jump_multiplier(&self) -> f64 {
        let entity = self.get_entity();
        let control = self
            .mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut power: f32 = if control.speed_modifier() <= 0.6 {
            0.2
        } else {
            0.3
        };
        let high_target = self
            .mob_entity
            .living_entity
            .jumping
            .load(Ordering::Relaxed)
            && control.wanted_position().y > entity.pos.load().y + 0.5;
        drop(control);
        let high_path = self
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_path()
            .and_then(|path| path.get_next_entity_pos(entity.entity_dimension.load().width))
            .is_some_and(|next| next.y > entity.pos.load().y + 0.5);
        if high_target || high_path || entity.horizontal_collision.load(Ordering::Relaxed) {
            power = 0.5;
        }
        f64::from(power / 0.42)
    }

    fn on_jump(&self) {
        let entity = self.get_entity();
        let speed = self
            .mob_entity
            .move_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .speed_modifier();
        let velocity = entity.velocity.load();
        if speed > 0.0 && velocity.x * velocity.x + velocity.z * velocity.z < 0.01 {
            entity.update_velocity_from_input(
                Vector3::new(0.0, if self.is_baby() { 0.5 } else { 1.5 }, 1.0),
                f64::from(0.1f32),
            );
        }
        entity
            .world
            .load()
            .send_entity_status(entity, EntityStatus::Jump, None);
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if self.jump_delay.load(Ordering::Relaxed) > 0 {
            self.jump_delay.fetch_sub(1, Ordering::Relaxed);
        }
        if self.more_carrot_ticks.load(Ordering::Relaxed) > 0 {
            let next =
                (self.more_carrot_ticks.load(Ordering::Relaxed) - rand::random_range(0..3)).max(0);
            self.more_carrot_ticks.store(next, Ordering::Relaxed);
        }
        let grounded = self.get_entity().on_ground.load(Ordering::Relaxed);
        if grounded {
            if !self.was_on_ground.load(Ordering::Relaxed) {
                self.mob_entity
                    .living_entity
                    .jumping
                    .store(false, Ordering::Relaxed);
                let speed = self
                    .mob_entity
                    .move_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .speed_modifier();
                self.jump_delay
                    .store(if speed < 2.2 { 10 } else { 3 }, Ordering::Relaxed);
                self.jump_can_control.store(false, Ordering::Relaxed);
            }
            if self.get_variant() == RabbitVariant::Evil
                && self.jump_delay.load(Ordering::Relaxed) == 0
                && let Some(target) = self.mob_entity.get_target()
                && self
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&target.get_entity().pos.load())
                    < 16.0
            {
                let pos = target.get_entity().pos.load();
                self.face_point(pos);
                let mut control = self
                    .mob_entity
                    .move_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let speed = control.speed_modifier();
                control.set_wanted_position(pos.x, pos.y, pos.z, speed);
                drop(control);
                self.start_jumping();
            }
            if !self.jump_requested.load(Ordering::Relaxed) {
                let control = self
                    .mob_entity
                    .move_control
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if control.has_wanted() && self.jump_delay.load(Ordering::Relaxed) == 0 {
                    let wanted = control.wanted_position();
                    drop(control);
                    let next = self
                        .mob_entity
                        .navigator
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .get_path()
                        .and_then(|path| {
                            path.get_next_entity_pos(
                                self.get_entity().entity_dimension.load().width,
                            )
                        });
                    self.face_point(next.unwrap_or(wanted));
                    self.start_jumping();
                }
            } else if !self.jump_can_control.load(Ordering::Relaxed) {
                self.jump_can_control.store(true, Ordering::Relaxed);
            }
        }
        self.was_on_ground.store(grounded, Ordering::Relaxed);
    }

    fn post_tick(&self) {
        let duration = self.jump_duration.load(Ordering::Relaxed);
        if self.jump_ticks.load(Ordering::Relaxed) != duration {
            self.jump_ticks.fetch_add(1, Ordering::Relaxed);
        } else if duration != 0 {
            self.jump_ticks.store(0, Ordering::Relaxed);
            self.jump_duration.store(0, Ordering::Relaxed);
            self.mob_entity
                .living_entity
                .jumping
                .store(false, Ordering::Relaxed);
        }
    }

    fn prepare_melee_attack(&self, damage: f32) -> f32 {
        if self.get_variant() == RabbitVariant::Evil && !self.get_entity().is_silent() {
            let entity = self.get_entity();
            entity.world.load().play_sound_fine(
                Sound::EntityRabbitAttack,
                SoundCategory::Hostile,
                &entity.pos.load(),
                1.0,
                (rand::random::<f32>() - rand::random::<f32>()) * 0.2 + 1.0,
            );
        }
        damage
    }

    fn create_breed_offspring_with_partner(
        &self,
        world: &Arc<World>,
        partner: &dyn Mob,
    ) -> Option<Arc<dyn EntityBase>> {
        let baby = self.create_breed_offspring(world)?;
        let variant = if rand::random_range(0..20) == 0 {
            RabbitVariant::random_variant(self.get_entity())
        } else if rand::random::<bool>() {
            partner
                .cast_any()
                .downcast_ref::<Self>()
                .map_or(self.get_variant(), Self::get_variant)
        } else {
            self.get_variant()
        };
        if let Some(rabbit) = baby.cast_any().downcast_ref::<Self>() {
            rabbit.set_variant(variant);
        }
        Some(baby)
    }
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }

    fn as_animal(&self) -> Option<&dyn Animal> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        nbt.put_int("RabbitType", self.get_variant().id());
        nbt.put_int(
            "MoreCarrotTicks",
            self.more_carrot_ticks.load(Ordering::Relaxed),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        if let Some(rabbit_type) = nbt.get_int("RabbitType") {
            self.set_variant(RabbitVariant::from_id(rabbit_type));
        }
        if let Some(ticks) = nbt.get_int("MoreCarrotTicks") {
            self.more_carrot_ticks.store(ticks, Ordering::Relaxed);
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ageable_ai_step();
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::rabbit::DATA_BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::rabbit::DATA_TYPE_ID,
            VarInt(self.get_variant().id()),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack, Sound::EntityRabbitAmbient)
    }
}
