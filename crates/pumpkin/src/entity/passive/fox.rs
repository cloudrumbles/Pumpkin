use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicI32, AtomicU8, Ordering},
};

use crate::world::World;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::Sound;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::codec::var_int::VarInt;
use uuid::Uuid;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        avoid_entity::AvoidEntityGoal,
        breed::BreedGoal,
        escape_danger::EscapeDangerGoal,
        follow_parent::FollowParentGoal,
        fox_behavior::{
            FoxDefendTrustedGoal, FoxEatBerriesGoal, FoxHuntTargetGoal, FoxPounceGoal,
            FoxSleepGoal, FoxStalkGoal,
        },
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal,
        ocelot_attack::OcelotAttackGoal,
        swim::SwimGoal,
        tempt::TemptGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

pub const FOX_FOOD: &[&Item] = &[&Item::SWEET_BERRIES, &Item::GLOW_BERRIES];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum FoxVariant {
    #[default]
    Red = 0,
    Snow = 1,
}

impl FoxVariant {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::Snow,
            _ => Self::Red,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Snow => "snow",
        }
    }

    #[must_use]
    pub fn from_name(s: &str) -> Self {
        match s {
            "snow" | "minecraft:snow" => Self::Snow,
            _ => Self::Red,
        }
    }
}

/// Represents a Fox, a passive/neutral mob.
///
/// Wiki: <https://minecraft.wiki/w/Fox>
pub struct FoxEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub variant: AtomicI32,
    pub flags: AtomicU8,
    pub trusted: Mutex<Vec<Uuid>>,
}

impl FoxEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let fox = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            variant: AtomicI32::new(FoxVariant::Red.id()),
            flags: AtomicU8::new(0),
            trusted: Mutex::new(Vec::new()),
        };
        let mob_arc = Arc::new(fox);
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
                4,
                Box::new(AvoidEntityGoal::with_filter(
                    16.0,
                    1.6,
                    1.4,
                    |mob, other| {
                        let Some(fox) = mob.cast_any().downcast_ref::<FoxEntity>() else {
                            return false;
                        };
                        other.get_entity().entity_type == &EntityType::PLAYER
                            && !fox.trusts(other.get_entity().entity_uuid)
                    },
                )),
            );
            goal_selector.add_goal(
                4,
                Box::new(AvoidEntityGoal::new(&EntityType::WOLF, 8.0, 1.6, 1.4)),
            );
            goal_selector.add_goal(6, Box::new(FoxStalkGoal));
            goal_selector.add_goal(5, Box::new(FoxPounceGoal::new()));
            goal_selector.add_goal(7, Box::new(OcelotAttackGoal::new()));
            goal_selector.add_goal(7, Box::new(FoxSleepGoal));
            goal_selector.add_goal(10, Box::new(FoxEatBerriesGoal::new()));
            goal_selector.add_goal(1, EscapeDangerGoal::new(1.5));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(3, Box::new(TemptGoal::new(1.2, FOX_FOOD, false)));
            goal_selector.add_goal(4, Box::new(FollowParentGoal::new(1.1)));
            goal_selector.add_goal(6, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                7,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));
            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            target_selector.add_goal(4, Box::new(FoxHuntTargetGoal::new()));
            target_selector.add_goal(3, Box::new(FoxDefendTrustedGoal::new()));
        };

        mob_arc
    }

    #[must_use]
    pub fn get_variant(&self) -> FoxVariant {
        FoxVariant::from_id(self.variant.load(Ordering::Relaxed))
    }

    pub fn set_variant(&self, variant: FoxVariant) {
        self.variant.store(variant.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::fox::TYPE_ID,
            VarInt(variant.id()),
        );
    }

    fn get_flag(&self, flag: u8) -> bool {
        (self.flags.load(Ordering::Relaxed) & flag) != 0
    }

    fn set_flag(&self, flag: u8, value: bool) {
        let current = self.flags.load(Ordering::Relaxed);
        let new_flags = if value {
            current | flag
        } else {
            current & !flag
        };
        self.flags.store(new_flags, Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(pumpkin_data::tracked_data::fox::FLAGS_ID, new_flags);
    }

    #[must_use]
    pub fn is_sitting(&self) -> bool {
        self.get_flag(1)
    }

    pub fn set_sitting(&self, val: bool) {
        self.set_flag(1, val);
    }

    #[must_use]
    pub fn is_crouching(&self) -> bool {
        self.get_flag(4)
    }

    pub fn set_crouching(&self, val: bool) {
        self.set_flag(4, val);
    }

    #[must_use]
    pub fn is_interested(&self) -> bool {
        self.get_flag(8)
    }

    pub fn set_interested(&self, val: bool) {
        self.set_flag(8, val);
    }

    #[must_use]
    pub fn is_pouncing(&self) -> bool {
        self.get_flag(16)
    }

    pub fn set_pouncing(&self, val: bool) {
        self.set_flag(16, val);
    }

    #[must_use]
    pub fn is_sleeping(&self) -> bool {
        self.get_flag(32)
    }

    pub fn set_sleeping(&self, val: bool) {
        self.set_flag(32, val);
    }

    #[must_use]
    pub fn is_faceplanted(&self) -> bool {
        self.get_flag(64)
    }

    pub fn set_faceplanted(&self, val: bool) {
        self.set_flag(64, val);
    }

    #[must_use]
    pub fn is_defending(&self) -> bool {
        self.get_flag(128)
    }

    pub fn set_defending(&self, val: bool) {
        self.set_flag(128, val);
    }

    pub fn trusts(&self, uuid: Uuid) -> bool {
        self.trusted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&uuid)
    }

    pub fn add_trusted(&self, uuid: Uuid) {
        let mut trusted = self
            .trusted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !trusted.contains(&uuid) && trusted.len() < 2 {
            trusted.push(uuid);
        }
    }

    pub fn clear_states(&self) {
        self.set_interested(false);
        self.set_crouching(false);
        self.set_sitting(false);
        self.set_sleeping(false);
        self.set_defending(false);
        self.set_faceplanted(false);
    }
}

impl AgeableMob for FoxEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}

impl Animal for FoxEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_FOX_FOOD)
            || FOX_FOOD.iter().any(|i| i.id == item_stack.item.id)
    }
}

impl Mob for FoxEntity {
    fn create_breed_offspring_with_partner(
        &self,
        world: &Arc<World>,
        partner: &dyn Mob,
    ) -> Option<Arc<dyn EntityBase>> {
        let baby = self.create_breed_offspring(world)?;
        if let Some(child) = baby.cast_any().downcast_ref::<FoxEntity>() {
            let other = partner.cast_any().downcast_ref::<FoxEntity>();
            let variant = if rand::random::<bool>() {
                self.get_variant()
            } else {
                other.map_or(self.get_variant(), FoxEntity::get_variant)
            };
            child.set_variant(variant);
            if let Some(breeder) = self.mob_entity.breeder.load() {
                child.add_trusted(breeder);
            }
            if let Some(breeder) = other.and_then(|p| p.mob_entity.breeder.load()) {
                child.add_trusted(breeder);
            }
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
        nbt.put_string("Type", self.get_variant().as_str().to_string());
        nbt.put_bool("Sleeping", self.is_sleeping());
        nbt.put_bool("Sitting", self.is_sitting());
        nbt.put_bool("Crouching", self.is_crouching());
        let list = self
            .trusted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|id| {
                let (hi, lo) = id.as_u64_pair();
                NbtTag::IntArray(vec![
                    (hi >> 32) as i32,
                    hi as i32,
                    (lo >> 32) as i32,
                    lo as i32,
                ])
            })
            .collect();
        nbt.put("Trusted", NbtTag::List(list));
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        if let Some(variant_str) = nbt.get_string("Type") {
            self.set_variant(FoxVariant::from_name(variant_str));
        }
        if let Some(sleeping) = nbt.get_bool("Sleeping") {
            self.set_sleeping(sleeping);
        }
        if let Some(sitting) = nbt.get_bool("Sitting") {
            self.set_sitting(sitting);
        }
        if let Some(crouching) = nbt.get_bool("Crouching") {
            self.set_crouching(crouching);
        }
        if let Some(list) = nbt.get_list("Trusted") {
            let mut trusted = self
                .trusted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trusted.clear();
            for tag in list {
                if let NbtTag::IntArray(values) = tag
                    && values.len() == 4
                {
                    let uuid = Uuid::from_u128(
                        (values[0] as u32 as u128) << 96
                            | (values[1] as u32 as u128) << 64
                            | (values[2] as u32 as u128) << 32
                            | values[3] as u32 as u128,
                    );
                    if !trusted.contains(&uuid) && trusted.len() < 2 {
                        trusted.push(uuid);
                    }
                }
            }
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_set_variant_name(&self, name: &str) {
        self.set_variant(FoxVariant::from_name(name));
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ageable_ai_step();
    }

    fn mob_init_data_tracker(&self) {
        let entity = self.get_entity();
        let is_baby = entity.age.load(Ordering::Relaxed) < 0;
        if is_baby {
            entity.set_synced_data(pumpkin_data::tracked_data::fox::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::fox::TYPE_ID,
            VarInt(self.get_variant().id()),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::fox::FLAGS_ID,
            self.flags.load(Ordering::Relaxed),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack, Sound::EntityFoxAmbient)
    }
}
