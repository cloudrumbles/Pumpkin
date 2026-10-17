use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, Ordering},
};

use crate::entity::ai::{
    brain::{
        Brain,
        memory::{GlobalPos, PackedMemories, types},
    },
    goal::{Controls, Goal, escape_danger::EscapeDangerGoal},
    pathfinder::NavigatorGoal,
};
use pumpkin_data::entity::{EntityStatus, EntityType};
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::entity::{
    Entity, EntityBase,
    ageable::{AgeableData, AgeableMob},
    ai::goal::{
        breed::BreedGoal, follow_parent::FollowParentGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, swim::SwimGoal, tempt::TemptGoal,
        wander_around::WanderAroundGoal,
    },
    item::ItemEntity,
    mob::{Mob, MobEntity},
    passive::animal::Animal,
    player::Player,
};

pub const SNIFFER_FOOD: &[&Item] = &[&Item::TORCHFLOWER_SEEDS];
pub const SNIFFER_BABY_START_AGE: i32 = -48000;
pub const DIGGING_DROP_SEED_OFFSET_TICKS: i32 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum SnifferState {
    #[default]
    Idling = 0,
    FeelingHappy = 1,
    Scenting = 2,
    Sniffing = 3,
    Searching = 4,
    Digging = 5,
    Rising = 6,
}

impl SnifferState {
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::FeelingHappy,
            2 => Self::Scenting,
            3 => Self::Sniffing,
            4 => Self::Searching,
            5 => Self::Digging,
            6 => Self::Rising,
            _ => Self::Idling,
        }
    }

    #[must_use]
    pub const fn id(self) -> i32 {
        self as i32
    }
}

/// Represents a Sniffer, a large passive mob that digs up ancient seeds.
///
/// Wiki: <https://minecraft.wiki/w/Sniffer>
pub struct SnifferEntity {
    pub mob_entity: MobEntity,
    pub ageable_data: AgeableData,
    pub state: AtomicI32,
    pub drop_seed_at_tick: AtomicI32,
}

impl SnifferEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let sniffer = Self {
            mob_entity,
            ageable_data: AgeableData::default(),
            state: AtomicI32::new(SnifferState::Idling.id()),
            drop_seed_at_tick: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(sniffer);
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
            goal_selector.add_goal(1, EscapeDangerGoal::new(2.0));
            goal_selector.add_goal(1, BreedGoal::new(1.0));
            goal_selector.add_goal(2, Box::new(TemptGoal::new(1.2, SNIFFER_FOOD, false)));
            goal_selector.add_goal(3, Box::new(FollowParentGoal::new(1.1)));
            goal_selector.add_goal(4, Box::new(SnifferForageGoal::default()));
            goal_selector.add_goal(5, Box::new(WanderAroundGoal::new(1.0)));
            goal_selector.add_goal(
                5,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 6.0),
            );
            goal_selector.add_goal(6, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }

    #[must_use]
    pub fn get_state(&self) -> SnifferState {
        SnifferState::from_id(self.state.load(Ordering::Relaxed))
    }

    pub fn set_state(&self, state: SnifferState) {
        self.state.store(state.id(), Ordering::Relaxed);
        let entity = self.get_entity();
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::STATE,
            VarInt(state.id()),
        );
    }

    pub fn transition_to(&self, state: SnifferState) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();

        match state {
            SnifferState::Idling => {
                self.set_state(SnifferState::Idling);
            }
            SnifferState::FeelingHappy => {
                world.play_sound(Sound::EntitySnifferHappy, SoundCategory::Neutral, &pos);
                self.set_state(SnifferState::FeelingHappy);
            }
            SnifferState::Scenting => {
                self.set_state(SnifferState::Scenting);
                self.on_scenting_start();
            }
            SnifferState::Sniffing => {
                world.play_sound(Sound::EntitySnifferSniffing, SoundCategory::Neutral, &pos);
                self.set_state(SnifferState::Sniffing);
            }
            SnifferState::Searching => {
                self.set_state(SnifferState::Searching);
            }
            SnifferState::Digging => {
                self.set_state(SnifferState::Digging);
                self.on_digging_start();
            }
            SnifferState::Rising => {
                world.play_sound(
                    Sound::EntitySnifferDiggingStop,
                    SoundCategory::Neutral,
                    &pos,
                );
                self.set_state(SnifferState::Rising);
            }
        }
    }

    fn on_scenting_start(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();
        let pitch = if self.is_baby() { 1.3 } else { 1.0 };
        world.play_sound_fine(
            Sound::EntitySnifferScenting,
            SoundCategory::Neutral,
            &pos,
            1.0,
            pitch,
        );
    }

    fn on_digging_start(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let current_ticks = world.level_time.try_lock().map_or(0, |t| t.world_age);
        let drop_tick = current_ticks as i32 + DIGGING_DROP_SEED_OFFSET_TICKS;
        self.drop_seed_at_tick.store(drop_tick, Ordering::Relaxed);
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::DROP_SEED_AT_TICK,
            VarInt(drop_tick),
        );
        world.send_entity_status(entity, EntityStatus::SnifferDiggingSound, None);
    }

    pub fn on_digging_complete(&self, success: bool) {
        if success {
            let head_block = self.get_head_block().down();
            self.store_explored_position(head_block);
        }
    }

    #[must_use]
    pub fn get_head_position(&self) -> Vector3<f64> {
        let entity = self.get_entity();
        let pos = entity.pos.load();
        let yaw = f64::from(entity.yaw.load());
        let yaw_rad = yaw.to_radians();
        let forward = Vector3::new(-yaw_rad.sin(), 0.0, yaw_rad.cos());
        pos + forward * 2.25
    }

    #[must_use]
    pub fn get_head_block(&self) -> BlockPos {
        let head_pos = self.get_head_position();
        let entity = self.get_entity();
        let pos = entity.pos.load();
        BlockPos::floored(head_pos.x, pos.y + 0.2, head_pos.z)
    }

    #[must_use]
    pub fn can_dig(&self) -> bool {
        let entity = self.get_entity();
        !self.is_panicking()
            && !self.is_baby()
            && !self.mob_entity.living_entity.is_in_water()
            && entity.on_ground.load(Ordering::Relaxed)
            && self.can_dig_at(self.get_head_block().down())
    }

    fn can_dig_at(&self, pos: BlockPos) -> bool {
        let entity = self.get_entity();
        let world = entity.world.load();
        let block_state = world.get_block_state(&pos);
        let block = pumpkin_data::Block::from_state_id(block_state.id);

        if !block.has_tag(&tag::Block::MINECRAFT_SNIFFER_DIGGABLE_BLOCK) {
            return false;
        }

        let brain = self
            .mob_entity
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !brain
            .get(types::SNIFFER_EXPLORED_POSITIONS)
            .is_some_and(|positions| {
                positions.iter().any(|p| {
                    p.dimension.minecraft_name == world.dimension.minecraft_name && p.pos == pos
                })
            })
    }

    fn store_explored_position(&self, pos: BlockPos) {
        let world = self.get_entity().world.load();
        let mut brain = self
            .mob_entity
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut positions = brain
            .get(types::SNIFFER_EXPLORED_POSITIONS)
            .cloned()
            .unwrap_or_default();
        positions.insert(
            0,
            GlobalPos::new(
                pumpkin_data::dimension::Dimension::from_name(world.dimension.minecraft_name)
                    .unwrap_or(&pumpkin_data::dimension::Dimension::OVERWORLD),
                pos,
            ),
        );
        positions.truncate(20);
        brain.set(types::SNIFFER_EXPLORED_POSITIONS, positions);
    }

    pub fn drop_seed(&self) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let current_tick = world.get_world_age() as i32;

        let deadline = self.drop_seed_at_tick.load(Ordering::Relaxed);
        if deadline > 0
            && current_tick >= deadline
            && self
                .drop_seed_at_tick
                .compare_exchange(deadline, 0, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            let head_pos = self.get_head_position();
            let seed_item = if rand::random::<bool>() {
                &Item::TORCHFLOWER_SEEDS
            } else {
                &Item::PITCHER_POD
            };
            let item_stack = ItemStack::new(1, seed_item);

            let item_entity = Entity::new(world.clone(), head_pos, &EntityType::ITEM);
            let item_arc = Arc::new(ItemEntity::new(item_entity, item_stack));
            world.spawn_entity(item_arc as Arc<dyn EntityBase>);

            world.play_sound(
                Sound::EntitySnifferDropSeed,
                SoundCategory::Neutral,
                &head_pos,
            );
        }
    }

    pub fn spawn_child_from_breeding(&self, partner: &dyn EntityBase) {
        let entity = self.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();

        let item_stack = ItemStack::new(1, &Item::SNIFFER_EGG);
        let egg_entity = Entity::new(world.clone(), pos, &EntityType::ITEM);
        let egg_arc = Arc::new(ItemEntity::new(egg_entity, item_stack));
        world.spawn_entity_non_save(egg_arc as Arc<dyn EntityBase>);

        world.play_sound(Sound::BlockSnifferEggPlop, SoundCategory::Neutral, &pos);

        self.mob_entity.reset_love_ticks();
        self.mob_entity
            .breeding_cooldown
            .store(6000, Ordering::Relaxed);
        partner.reset_love();
        partner.set_breeding_cooldown(6000);
    }
}

impl AgeableMob for SnifferEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }

    fn get_baby_start_age(&self) -> i32 {
        SNIFFER_BABY_START_AGE
    }
}

impl Animal for SnifferEntity {
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        item_stack.item.has_tag(&tag::Item::MINECRAFT_SNIFFER_FOOD)
            || SNIFFER_FOOD.iter().any(|i| i.id == item_stack.item.id)
    }
}

impl Mob for SnifferEntity {
    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        let mut brain = Brain::default();
        brain.register_memory(types::SNIFFER_EXPLORED_POSITIONS.id());
        brain.register_memory(types::SNIFF_COOLDOWN.id());
        brain.load_packed(packed);
        brain
    }
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let world = self.get_entity().world.load_full();
        self.mob_entity
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tick(&world, self, world.get_world_age());
    }
    fn create_breed_offspring(
        &self,
        world: &Arc<crate::world::World>,
    ) -> Option<Arc<dyn EntityBase>> {
        let egg = Arc::new(ItemEntity::new(
            Entity::new(
                world.clone(),
                self.get_entity().pos.load(),
                &EntityType::ITEM,
            ),
            ItemStack::new(1, &Item::SNIFFER_EGG),
        ));
        self.get_entity().play_sound(Sound::BlockSnifferEggPlop);
        Some(egg)
    }

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
        // Java saves explored sites and cooldown in Brain, not transient animations.
        self.set_state(SnifferState::Idling);
        self.drop_seed_at_tick.store(0, Ordering::Relaxed);
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
            entity.set_synced_data(pumpkin_data::tracked_data::sniffer::BABY_ID, true);
        }
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::STATE,
            VarInt(self.get_state().id()),
        );
        entity.set_synced_data(
            pumpkin_data::tracked_data::sniffer::DROP_SEED_AT_TICK,
            VarInt(self.drop_seed_at_tick.load(Ordering::Relaxed)),
        );
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        self.animal_interact(player, item_stack, Sound::EntitySnifferEat)
    }
}

#[derive(Default)]
struct SnifferForageGoal {
    remaining: i32,
    elapsed: i32,
    destination: Option<Vector3<f64>>,
    retry: i64,
}
impl SnifferForageGoal {
    fn sniffer(mob: &dyn Mob) -> Option<&SnifferEntity> {
        mob.cast_any().downcast_ref::<SnifferEntity>()
    }
    fn change(&mut self, sniffer: &SnifferEntity, state: SnifferState, ticks: i32) {
        self.remaining = ticks;
        self.elapsed = 0;
        sniffer.transition_to(state);
    }
    fn select_site(&mut self, sniffer: &SnifferEntity) -> bool {
        if sniffer.can_dig() {
            self.destination = Some(sniffer.get_entity().pos.load());
            return true;
        }
        let entity = sniffer.get_entity();
        let world = entity.world.load();
        let origin = entity.pos.load();
        for _ in 0..40 {
            let dx = rand::random_range(-14..=14);
            let dz = rand::random_range(-14..=14);
            for dy in (-3..=3).rev() {
                let foot = BlockPos::floored_v(origin).offset(Vector3::new(dx, dy, dz));
                let dest = foot.to_f64() + Vector3::new(0.5, 0.0, 0.5);
                let direction = Vector3::new(dest.x - origin.x, 0.0, dest.z - origin.z).normalize();
                let head = BlockPos::floored_v(dest + direction * 2.25).down();
                if world.get_block(&foot).is_air()
                    && world.get_block(&foot.down()).is_solid()
                    && sniffer.can_dig_at(head)
                    && sniffer
                        .mob_entity
                        .navigator
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .can_reach_within(&sniffer.mob_entity.living_entity, dest, 0.5)
                {
                    self.destination = Some(dest);
                    return true;
                }
            }
        }
        false
    }
}
impl Goal for SnifferForageGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(s) = Self::sniffer(mob) else {
            return false;
        };
        let entity = mob.get_entity();
        let now = entity.world.load().get_world_age();
        if now < self.retry
            || s.is_panicking()
            || s.mob_entity.is_in_love()
            || entity.touching_water.load(Ordering::Relaxed)
            || !entity.on_ground.load(Ordering::Relaxed)
            || !mob.is_navigator_idle()
        {
            return false;
        }
        self.retry = now + 40;
        rand::random_range(0..3) == 0
    }
    fn start(&mut self, mob: &dyn Mob) {
        if let Some(s) = Self::sniffer(mob) {
            let scent = s.is_baby()
                || s.mob_entity
                    .brain
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .has_memory_value(types::SNIFF_COOLDOWN.id())
                || rand::random::<bool>();
            self.change(
                s,
                if scent {
                    SnifferState::Scenting
                } else {
                    SnifferState::Sniffing
                },
                rand::random_range(40..=80),
            );
        }
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        Self::sniffer(mob).is_some_and(|s| {
            self.remaining > 0
                && !s.is_panicking()
                && !s.mob_entity.is_in_love()
                && !s.get_entity().touching_water.load(Ordering::Relaxed)
                && (s.get_state() != SnifferState::Digging || s.can_dig())
        })
    }
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(s) = Self::sniffer(mob) else {
            return;
        };
        self.remaining -= 1;
        self.elapsed += 1;
        match s.get_state() {
            SnifferState::Sniffing if self.remaining <= 0 => {
                if self.select_site(s) {
                    self.change(s, SnifferState::Searching, 600);
                }
            }
            SnifferState::Searching => {
                let entity = mob.get_entity();
                if self.elapsed % 20 == 1 {
                    entity.play_sound(Sound::EntitySnifferSearching);
                }
                if let Some(dest) = self.destination {
                    if entity.pos.load().squared_distance_to_vec(&dest) < 0.75 && s.can_dig() {
                        s.mob_entity
                            .navigator
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .stop();
                        self.change(s, SnifferState::Digging, rand::random_range(160..=180));
                    } else if self.elapsed % 20 == 1 {
                        s.mob_entity
                            .navigator
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .set_progress(NavigatorGoal::new(entity.pos.load(), dest, 1.25));
                    }
                }
            }
            SnifferState::Digging => {
                // Offset counts AI ticks, so pausing with NoAI cannot duplicate a drop.
                if self.elapsed == 120 {
                    s.drop_seed_at_tick.store(
                        s.get_entity().world.load().get_world_age() as i32,
                        Ordering::Relaxed,
                    );
                    s.drop_seed();
                }
                if self.remaining <= 0 {
                    s.mob_entity
                        .brain
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .set_with_expiry(types::SNIFF_COOLDOWN, (), 9600);
                    self.change(s, SnifferState::Rising, 40);
                }
            }
            SnifferState::Rising if self.remaining <= 0 => {
                s.on_digging_complete(true);
                self.change(s, SnifferState::FeelingHappy, rand::random_range(40..=100));
            }
            _ => {}
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(s) = Self::sniffer(mob) {
            s.transition_to(SnifferState::Idling);
            s.drop_seed_at_tick.store(0, Ordering::Relaxed);
        }
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        self.destination = None;
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }
}
