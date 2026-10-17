use crate::entity::{
    Entity, EntityBase,
    ai::{
        goal::{
            Controls, Goal, escape_danger::EscapeDangerGoal, look_around::RandomLookAroundGoal,
            look_at_entity::LookAtEntityGoal, swim::SwimGoal,
            water_avoiding_random_flying::WaterAvoidingRandomFlyingGoal,
        },
        pathfinder::{Navigator, NavigatorGoal},
    },
    item::ItemEntity,
    mob::{Mob, MobEntity},
    player::Player,
};
use crossbeam::atomic::AtomicCell;
use pumpkin_data::{
    Block,
    damage::DamageType,
    data_component_impl::{EquipmentSlot, PotionContentsImpl},
    entity::EntityType,
    item_stack::ItemStack,
    particle::Particle,
    sound::{Sound, SoundCategory},
    tag::{self, Taggable},
};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering},
};
use uuid::Uuid;

pub struct AllayEntity {
    pub mob_entity: MobEntity,
    pub dancing: AtomicBool,
    pub can_duplicate: AtomicBool,
    pub duplication_cooldown: AtomicI32,
    pub owner: AtomicCell<Option<Uuid>>,
    inventory: Mutex<ItemStack>,
    note_block: AtomicCell<Option<BlockPos>>,
    note_expires: AtomicI64,
    jukebox: AtomicCell<Option<BlockPos>>,
    pickup_after: AtomicI64,
    next_throw: AtomicI64,
    dropped_inventory: AtomicBool,
}

impl AllayEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        *mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Navigator::flying();
        mob_entity
            .living_entity
            .equipment_drop_chances
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(EquipmentSlot::MAIN_HAND, 2.0);
        let allay = Arc::new(Self {
            mob_entity,
            dancing: AtomicBool::new(false),
            can_duplicate: AtomicBool::new(true),
            duplication_cooldown: AtomicI32::new(0),
            owner: AtomicCell::new(None),
            inventory: Mutex::new(ItemStack::EMPTY.clone()),
            note_block: AtomicCell::new(None),
            note_expires: AtomicI64::new(0),
            jukebox: AtomicCell::new(None),
            pickup_after: AtomicI64::new(0),
            next_throw: AtomicI64::new(0),
            dropped_inventory: AtomicBool::new(false),
        });
        let weak: Weak<dyn Mob> = Arc::downgrade(&(allay.clone() as Arc<dyn Mob>));
        {
            let mut goals = allay
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            goals.add_goal(0, Box::new(SwimGoal::default()));
            goals.add_goal(1, EscapeDangerGoal::new(2.5));
            goals.add_goal(
                2,
                Box::new(AllayWorkGoal {
                    allay: Arc::downgrade(&allay),
                }),
            );
            goals.add_goal(6, Box::new(WaterAvoidingRandomFlyingGoal::new(1.0)));
            goals.add_goal(
                7,
                LookAtEntityGoal::with_default(weak, &EntityType::PLAYER, 6.0),
            );
            goals.add_goal(8, Box::new(RandomLookAroundGoal::default()));
        }
        allay
    }

    pub fn is_dancing(&self) -> bool {
        self.dancing.load(Ordering::Relaxed)
    }
    pub fn set_dancing(&self, dancing: bool) {
        self.dancing.store(dancing, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(pumpkin_data::tracked_data::allay::DATA_DANCING, dancing);
    }
    pub fn can_duplicate(&self) -> bool {
        self.can_duplicate.load(Ordering::Relaxed)
    }
    pub fn set_can_duplicate(&self, value: bool) {
        self.can_duplicate.store(value, Ordering::Relaxed);
        self.get_entity()
            .set_synced_data(pumpkin_data::tracked_data::allay::DATA_CAN_DUPLICATE, value);
    }
    fn held_item(&self) -> ItemStack {
        self.mob_entity.living_entity.held_item(self)
    }
    fn set_held_item(&self, stack: ItemStack) {
        let living = &self.mob_entity.living_entity;
        living
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .put(&EquipmentSlot::MAIN_HAND, stack.clone());
        living.send_equipment_changes(&[(EquipmentSlot::MAIN_HAND, stack)]);
    }

    /// Called by the world's game-event dispatch after plugin cancellation.
    pub fn hear_game_event(&self, key: &str, position: Vector3<f64>) {
        let entity = self.get_entity();
        if self.mob_entity.is_no_ai() {
            return;
        }
        let distance = entity.pos.load().squared_distance_to_vec(&position);
        let pos = BlockPos::floored_v(position);
        match key {
            "note_block_play" if distance <= 256.0 => {
                let now = entity.world.load().get_world_age();
                if self.note_expires.load(Ordering::Relaxed) <= now
                    || self.note_block.load().is_none_or(|old| old == pos)
                {
                    self.note_block.store(Some(pos));
                    self.note_expires.store(now + 600, Ordering::Relaxed);
                }
            }
            "jukebox_play" if distance <= 100.0 => {
                self.jukebox.store(Some(pos));
                self.set_dancing(true);
            }
            "jukebox_stop_play" if self.jukebox.load() == Some(pos) => {
                self.jukebox.store(None);
                self.set_dancing(false);
            }
            _ => {}
        }
    }

    fn deposit_position(&self) -> Option<Vector3<f64>> {
        let entity = self.get_entity();
        let world = entity.world.load();
        if let Some(note) = self.note_block.load() {
            if self.note_expires.load(Ordering::Relaxed) > world.get_world_age()
                && world.get_block(&note).id == Block::NOTE_BLOCK.id
                && entity
                    .pos
                    .load()
                    .squared_distance_to_vec(&note.to_centered_f64())
                    < 1024.0 * 1024.0
            {
                return Some(note.up().to_centered_f64());
            }
            self.note_block.store(None);
        }
        let owner = world.get_player_by_uuid(self.owner.load()?)?;
        (!owner.is_spectator()
            && entity
                .pos
                .load()
                .squared_distance_to_vec(&owner.get_entity().pos.load())
                < 4096.0)
            .then(|| owner.get_entity().pos.load() + Vector3::new(0.0, 1.0, 0.0))
    }

    fn wants(&self, stack: &ItemStack) -> bool {
        let held = self.held_item();
        if held.is_empty() || stack.is_empty() || held.item.id != stack.item.id {
            return false;
        }
        if held.get_data_component::<PotionContentsImpl>()
            != stack.get_data_component::<PotionContentsImpl>()
        {
            return false;
        }
        let carried = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        carried.is_empty()
            || (carried.are_items_and_components_equal(stack)
                && carried.item_count < carried.get_max_stack_size())
    }

    fn wanted_item(&self) -> Option<Arc<dyn EntityBase>> {
        let entity = self.get_entity();
        let world = entity.world.load();
        if world.get_world_age() < self.pickup_after.load(Ordering::Relaxed)
            || !world.level_info.load().game_rules.mob_griefing
        {
            return None;
        }
        world.get_nearest_entity(
            entity.pos.load(),
            32.0,
            Some(&[&EntityType::ITEM]),
            |target| {
                target.get_item_entity().is_some_and(|item| {
                    item.get_pickup_delay() == 0
                        && self.wants(
                            &item
                                .get_item_stack()
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner),
                        )
                }) && self.has_line_of_sight(target.get_entity())
            },
        )
    }

    fn throw_stack(&self, stack: ItemStack, target: Vector3<f64>) {
        if stack.is_empty() {
            return;
        }
        let entity = self.get_entity();
        let origin = entity.pos.load() + Vector3::new(0.0, entity.get_eye_height() - 0.2, 0.0);
        let delta = (target - origin).normalize();
        let world = entity.world.load();
        let item = ItemEntity::new_with_velocity(
            Entity::new(world.clone(), origin, &EntityType::ITEM),
            stack,
            Vector3::new(delta.x * 0.2, 0.3, delta.z * 0.2),
            10,
        );
        world.spawn_entity_non_save(Arc::new(item));
    }

    fn work_tick(&self) {
        let entity = self.get_entity();
        let now = entity.world.load().get_world_age();
        if let Some(target) = self.wanted_item() {
            let destination = target.get_entity().pos.load();
            if entity
                .bounding_box
                .load()
                .expand(1.0, 1.0, 1.0)
                .intersects(&target.get_entity().bounding_box.load())
            {
                let held = self.held_item();
                let limit = {
                    let carried = self
                        .inventory
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if carried.is_empty() {
                        64
                    } else {
                        carried
                            .get_max_stack_size()
                            .saturating_sub(carried.item_count)
                    }
                };
                if let Some(item) = target.get_item_entity()
                    && let Some(stack) =
                        item.take_for_mob(&self.mob_entity.living_entity, limit, |stack| {
                            stack.item.id == held.item.id && self.wants(stack)
                        })
                {
                    let mut carried = self
                        .inventory
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if carried.is_empty() {
                        *carried = stack;
                    } else {
                        carried.item_count += stack.item_count;
                    }
                }
            } else {
                self.navigate(destination, 1.75);
            }
            return;
        }
        if let Some(destination) = self.deposit_position() {
            let distance = entity.pos.load().squared_distance_to_vec(&destination);
            if distance > 9.0 {
                self.navigate(destination, 2.25);
            } else {
                self.mob_entity
                    .navigator
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stop();
                if now >= self.next_throw.load(Ordering::Relaxed) {
                    let stack = self
                        .inventory
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .split(1);
                    if !stack.is_empty() {
                        self.throw_stack(stack, destination);
                        self.pickup_after.store(now + 60, Ordering::Relaxed);
                        self.next_throw.store(now + 20, Ordering::Relaxed);
                        entity.play_sound(Sound::EntityAllayItemThrown);
                    }
                }
            }
        }
    }

    fn navigate(&self, target: Vector3<f64>, speed: f64) {
        self.mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal::new(
                self.get_entity().pos.load(),
                target,
                speed,
            ));
    }
}

struct AllayWorkGoal {
    allay: Weak<AllayEntity>,
}
impl Goal for AllayWorkGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        self.allay.upgrade().is_some_and(|allay| {
            !allay.is_dancing()
                && !allay.held_item().is_empty()
                && (allay.wanted_item().is_some() || allay.deposit_position().is_some())
        })
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob)
    }
    fn tick(&mut self, _mob: &dyn Mob) {
        if let Some(allay) = self.allay.upgrade() {
            allay.work_tick();
        }
    }
    fn stop(&mut self, mob: &dyn Mob) {
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
        Controls::MOVE | Controls::LOOK
    }
}

impl Mob for AllayEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
    fn get_mob_gravity(&self) -> f64 {
        0.0
    }
    fn get_mob_y_velocity_drag(&self) -> Option<f64> {
        Some(0.91)
    }
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        false
    }
    fn pre_damage(&self, _damage_type: DamageType, source: Option<&dyn EntityBase>) -> bool {
        !source.is_some_and(|source| Some(source.get_entity().entity_uuid) == self.owner.load())
    }
    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_bool("CanDuplicate", self.can_duplicate());
        nbt.put_long(
            "DuplicationCooldown",
            i64::from(self.duplication_cooldown.load(Ordering::Relaxed)),
        );
        if let Some(owner) = self.owner.load() {
            nbt.put_uuid("Owner", owner);
        }
        let carried = self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !carried.is_empty() {
            let mut stack = NbtCompound::new();
            carried.write_item_stack(&mut stack);
            nbt.put("Inventory", NbtTag::List(vec![NbtTag::Compound(stack)]));
        }
    }
    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        let cooldown = nbt
            .get_long("DuplicationCooldown")
            .or_else(|| nbt.get_int("DuplicationCooldown").map(i64::from))
            .unwrap_or(0)
            .clamp(0, i64::from(i32::MAX)) as i32;
        self.duplication_cooldown.store(cooldown, Ordering::Relaxed);
        self.set_can_duplicate(cooldown == 0);
        self.owner.store(nbt.get_uuid("Owner"));
        let carried = nbt
            .get_list("Inventory")
            .and_then(|list| list.first())
            .and_then(NbtTag::extract_compound)
            .and_then(ItemStack::read_item_stack)
            .unwrap_or_else(|| ItemStack::EMPTY.clone());
        *self
            .inventory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = carried;
    }
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();
        if entity.world.load().get_world_age() % 10 == 0 {
            self.mob_entity.living_entity.heal(1.0);
        }
        let cooldown = self.duplication_cooldown.load(Ordering::Relaxed);
        if cooldown > 0 {
            self.duplication_cooldown
                .store(cooldown - 1, Ordering::Relaxed);
            if cooldown == 1 {
                self.set_can_duplicate(true);
            }
        }
        if let Some(jukebox) = self.jukebox.load()
            && (entity
                .pos
                .load()
                .squared_distance_to_vec(&jukebox.to_centered_f64())
                > 100.0
                || entity.world.load().get_block(&jukebox).id != Block::JUKEBOX.id)
        {
            self.jukebox.store(None);
            self.set_dancing(false);
        }
    }
    fn post_tick(&self) {
        if self.mob_entity.living_entity.dead.load(Ordering::Relaxed)
            && !self.dropped_inventory.swap(true, Ordering::Relaxed)
        {
            let stack = std::mem::replace(
                &mut *self
                    .inventory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                ItemStack::EMPTY.clone(),
            );
            self.throw_stack(stack, self.get_entity().pos.load());
        }
    }
    fn mob_init_data_tracker(&self) {
        self.set_dancing(self.is_dancing());
        self.set_can_duplicate(self.can_duplicate());
    }
    fn mob_interact(&self, player: &Arc<Player>, stack: &mut ItemStack) -> bool {
        let entity = self.get_entity();
        if self.is_dancing()
            && self.can_duplicate()
            && stack.item.has_tag(&tag::Item::MINECRAFT_DUPLICATES_ALLAYS)
        {
            stack.decrement_unless_creative(player.gamemode.load(), 1);
            self.set_can_duplicate(false);
            self.duplication_cooldown.store(6000, Ordering::Relaxed);
            let world = entity.world.load();
            let child = Self::new(Entity::new(
                world.clone(),
                entity.pos.load(),
                &EntityType::ALLAY,
            ));
            child.set_can_duplicate(false);
            child.duplication_cooldown.store(6000, Ordering::Relaxed);
            world.spawn_entity_non_save(child);
            world.spawn_particle(
                entity.pos.load(),
                Vector3::new(0.5, 0.5, 0.5),
                1.0,
                7,
                Particle::Heart,
            );
            world.play_sound(
                Sound::BlockAmethystBlockChime,
                SoundCategory::Neutral,
                &entity.pos.load(),
            );
            return true;
        }
        let held = self.held_item();
        if held.is_empty() && !stack.is_empty() {
            let mut sample = stack.clone();
            sample.item_count = 1;
            self.set_held_item(sample);
            stack.decrement_unless_creative(player.gamemode.load(), 1);
            self.owner.store(Some(player.gameprofile.id));
            entity.play_sound(Sound::EntityAllayItemGiven);
            return true;
        }
        if !held.is_empty() && stack.is_empty() {
            self.set_held_item(ItemStack::EMPTY.clone());
            self.owner.store(None);
            *stack = held;
            let collected = std::mem::replace(
                &mut *self
                    .inventory
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                ItemStack::EMPTY.clone(),
            );
            self.throw_stack(collected, entity.pos.load());
            entity.play_sound(Sound::EntityAllayItemTaken);
            return true;
        }
        false
    }
}
