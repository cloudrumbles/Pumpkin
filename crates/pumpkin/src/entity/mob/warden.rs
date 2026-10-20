use crate::{
    entity::{
        Entity, EntityBase,
        ai::{
            brain::{
                Brain,
                memory::{MemoryModuleType, PackedMemories, types},
            },
            goal::{Controls, Goal, is_valid_attack_target, swim::SwimGoal},
            pathfinder::NavigatorGoal,
            util::land_random_pos,
            vibration,
        },
        mob::{Mob, MobEntity},
    },
    world::World,
};
use pumpkin_data::{
    attributes::Attributes,
    damage::DamageType,
    effect::StatusEffect,
    entity::{EntityPose, EntityStatus, EntityType},
    particle::Particle,
    potion::Effect,
    sound::Sound,
    tag::{self, Taggable},
};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::{GameMode, math::vector3::Vector3};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, Weak, atomic::Ordering},
};
use uuid::Uuid;

const EMERGE_DURATION: i32 = 134;
const ANGRY: i32 = 80;

#[derive(Clone, Copy, Default)]
enum Phase {
    #[default]
    Idle,
    Emerging(i32),
    Digging(i32),
    Sniffing(i32),
    Roaring(Uuid, i32),
    Sonic(Uuid, i32),
}
#[derive(Clone)]
struct Vibration {
    position: Vector3<f64>,
    source: Option<Uuid>,
    owner: Option<Uuid>,
    arrival: i64,
    distance: f64,
}
enum Stimulus {
    Vibration(Vibration),
    Damage(Uuid, bool),
}
#[derive(Clone, Default)]
struct WardenState {
    phase: Phase,
    anger: Vec<(Uuid, i32)>,
    pending: Option<Vibration>,
    disturbance: Option<(Vector3<f64>, i32)>,
    ticks: i64,
    attack_cooldown: i32,
}

pub struct WardenEntity {
    pub mob_entity: MobEntity,
    state: Mutex<WardenState>,
    stimuli: Mutex<VecDeque<Stimulus>>,
}

impl WardenEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let warden = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            state: Mutex::new(WardenState::default()),
            stimuli: Mutex::new(VecDeque::new()),
        });
        *warden
            .mob_entity
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            warden.make_brain(&PackedMemories::empty());
        {
            let mut goals = warden
                .mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            goals.add_goal(0, Box::new(SwimGoal::default()));
            goals.add_goal(
                1,
                Box::new(WardenBehavior {
                    warden: Arc::downgrade(&warden),
                }),
            );
        }
        warden
    }
    pub fn emerge(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .phase = Phase::Emerging(EMERGE_DURATION);
        self.remember(types::IS_EMERGING, EMERGE_DURATION.into());
        self.get_entity().set_pose(EntityPose::Emerging);
        self.get_entity().play_sound(Sound::EntityWardenAgitated);
    }
    fn valid(&self, target: &dyn EntityBase) -> bool {
        let entity = target.get_entity();
        is_valid_attack_target(self, target)
            && entity.entity_type != &EntityType::WARDEN
            && entity.entity_type != &EntityType::ARMOR_STAND
            && !entity.invulnerable.load(Ordering::Relaxed)
    }
    fn memory_present(&self, key: MemoryModuleType<()>) -> bool {
        self.mob_entity
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .has_memory_value(key.id())
    }
    fn remember(&self, key: MemoryModuleType<()>, ticks: i64) {
        self.mob_entity
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_with_expiry(key, (), ticks);
    }
    fn reset_dig(&self) {
        if self.memory_present(types::DIG_COOLDOWN) {
            self.remember(types::DIG_COOLDOWN, 1200);
        }
    }
    fn stop_moving(&self) {
        self.mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        self.mob_entity
            .living_entity
            .movement_input
            .store(Vector3::new(0.0, 0.0, 0.0));
    }
    fn navigate(&self, position: Vector3<f64>, speed: f64) {
        self.mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal::new(
                self.get_entity().pos.load(),
                position,
                speed,
            ));
    }
    fn queue(&self, stimulus: Stimulus) {
        let mut queue = self
            .stimuli
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if queue.len() >= 64 {
            queue.pop_front();
        }
        queue.push_back(stimulus);
    }
    fn increase_anger(&self, state: &mut WardenState, target: &dyn EntityBase, amount: i32) {
        if !self.valid(target) {
            return;
        }
        let uuid = target.get_entity().entity_uuid;
        if let Some((_, anger)) = state.anger.iter_mut().find(|(id, _)| *id == uuid) {
            *anger = (*anger + amount).min(150);
        } else {
            state.anger.push((uuid, amount.min(150)));
        }
        self.reset_dig();
    }
    fn set_attack_target(&self, target: Option<Arc<dyn EntityBase>>) {
        let mut brain = self
            .mob_entity
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        brain.set_optional(types::ATTACK_TARGET, target.clone());
        brain.erase(types::ROAR_TARGET.id());
        drop(brain);
        self.mob_entity.set_target(target);
        self.remember(types::SONIC_BOOM_COOLDOWN, 200);
    }
    pub fn hear_game_event(&self, key: &str, position: Vector3<f64>, source: Option<&Entity>) {
        if self.mob_entity.is_no_ai()
            || !self.mob_entity.living_entity.is_alive()
            || !tag::GameEvent::MINECRAFT_WARDEN_CAN_LISTEN.0.contains(&key)
            || matches!(
                self.get_entity().pose.load(),
                EntityPose::Emerging | EntityPose::Digging
            )
            || self.memory_present(types::VIBRATION_COOLDOWN)
        {
            return;
        }
        let entity = self.get_entity();
        let world = entity.world.load();
        let listener =
            entity.pos.load() + Vector3::new(0.0, f64::from(entity.get_eye_height()), 0.0);
        let distance = listener.squared_distance_to_vec(&position).sqrt();
        if distance > 16.0 {
            return;
        }
        let source_entity = source.and_then(|e| world.get_entity_by_id(e.entity_id));
        if source.is_some_and(|e| {
            e.entity_id == entity.entity_id
                || (e.is_sneaking()
                    && tag::GameEvent::MINECRAFT_IGNORE_VIBRATIONS_SNEAKING
                        .0
                        .contains(&key))
        }) {
            return;
        }
        if source_entity.as_ref().is_some_and(|e| {
            e.is_spectator() || (e.get_living_entity().is_some() && !self.valid(e.as_ref()))
        }) {
            return;
        }
        if vibration::is_occluded(&world, position, listener) {
            return;
        }
        let owner = source_entity
            .as_ref()
            .and_then(|e| e.get_owner_id())
            .and_then(|id| world.get_entity_by_id(id))
            .map(|e| e.get_entity().entity_uuid);
        self.queue(Stimulus::Vibration(Vibration {
            position,
            source: source.map(|e| e.entity_uuid),
            owner,
            arrival: world.get_world_age() + distance.floor() as i64,
            distance,
        }));
    }
    fn receive_vibration(&self, state: &mut WardenState, vibration: Vibration) {
        let entity = self.get_entity();
        let world = entity.world.load();
        self.remember(types::VIBRATION_COOLDOWN, 40);
        world.send_entity_status(entity, EntityStatus::TendrilsShiver, None);
        entity.play_sound(Sound::EntityWardenTendrilClicks);
        let mut suspicious = vibration.position;
        if let Some(owner) = vibration.owner {
            if let Some(target) = world.get_entity_by_uuid(owner)
                && target
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&entity.pos.load())
                    < 900.0
            {
                let repeated = self.memory_present(types::RECENT_PROJECTILE);
                self.increase_anger(state, target.as_ref(), if repeated { 35 } else { 10 });
                if repeated && self.valid(target.as_ref()) {
                    suspicious = target.get_entity().pos.load();
                }
            }
            self.remember(types::RECENT_PROJECTILE, 100);
        } else if let Some(source) = vibration
            .source
            .and_then(|uuid| world.get_entity_by_uuid(uuid))
        {
            self.increase_anger(state, source.as_ref(), 35);
        }
        state.disturbance = Some((suspicious, 100));
        self.remember(types::SNIFF_COOLDOWN, 100);
        self.reset_dig();
        entity.play_sound(if state.anger.iter().any(|(_, anger)| *anger >= ANGRY) {
            Sound::EntityWardenListeningAngry
        } else {
            Sound::EntityWardenListening
        });
    }
    fn nearest_suspect(&self) -> Option<Arc<dyn EntityBase>> {
        let entity = self.get_entity();
        let world = entity.world.load();
        // Smell ignores visibility and finds living entities in a horizontal range.
        world
            .get_all_at_box(&entity.bounding_box.load().expand(24.0, 20.0, 24.0))
            .into_iter()
            .filter(|e| self.valid(e.as_ref()))
            .min_by(|a, b| {
                a.get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&entity.pos.load())
                    .total_cmp(
                        &b.get_entity()
                            .pos
                            .load()
                            .squared_distance_to_vec(&entity.pos.load()),
                    )
            })
    }
    fn sonic_in_range(&self, target: &dyn EntityBase) -> bool {
        let delta = target.get_entity().pos.load() - self.get_entity().pos.load();
        delta.x * delta.x + delta.z * delta.z < 225.0 && delta.y.abs() < 20.0
    }
    fn sonic_boom(&self, target: &dyn EntityBase) {
        if !self.valid(target) || !self.sonic_in_range(target) {
            return;
        }
        let entity = self.get_entity();
        let world = entity.world.load();
        let source = entity.pos.load() + Vector3::new(0.0, 1.6, 0.0);
        let delta = target.get_entity().pos.load()
            + Vector3::new(0.0, f64::from(target.get_entity().get_eye_height()), 0.0)
            - source;
        let direction = delta.normalize();
        for step in 1..delta.length().floor() as i32 + 7 {
            world.spawn_particle(
                source + direction * f64::from(step),
                Vector3::new(0.0, 0.0, 0.0),
                0.0,
                1,
                Particle::SonicBoom,
            );
        }
        entity.play_sound(Sound::EntityWardenSonicBoom);
        if target.damage_with_context(
            target,
            10.0,
            DamageType::SONIC_BOOM,
            Some(source),
            Some(self),
            Some(self),
        ) && let Some(living) = target.get_living_entity()
        {
            let scale = 1.0 - living.get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE);
            target.get_entity().set_velocity(
                target.get_entity().velocity.load()
                    + Vector3::new(
                        direction.x * 2.5 * scale,
                        direction.y * 0.5 * scale,
                        direction.z * 2.5 * scale,
                    ),
            );
        }
    }
    fn tick_ai(&self) {
        let entity = self.get_entity();
        let world = entity.world.load_full();
        let now = world.get_world_age();
        self.mob_entity
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .tick(&world, self, now);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        state.ticks += 1;
        state.attack_cooldown = (state.attack_cooldown - 1).max(0);
        let stimuli = std::mem::take(
            &mut *self
                .stimuli
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for stimulus in stimuli {
            match stimulus {
                Stimulus::Damage(uuid, immediate) => {
                    if let Some(target) = world.get_entity_by_uuid(uuid) {
                        self.increase_anger(&mut state, target.as_ref(), 100);
                        if immediate && self.valid(target.as_ref()) {
                            self.set_attack_target(Some(target));
                            state.phase = Phase::Idle;
                            entity.set_pose(EntityPose::Standing);
                        }
                    }
                }
                Stimulus::Vibration(v) => {
                    if !self.memory_present(types::VIBRATION_COOLDOWN)
                        && state.pending.as_ref().is_none_or(|old| {
                            v.arrival < old.arrival
                                || (v.arrival == old.arrival && v.distance < old.distance)
                        })
                    {
                        state.pending = Some(v);
                    }
                }
            }
        }
        if state.pending.as_ref().is_some_and(|v| v.arrival <= now)
            && let Some(v) = state.pending.take()
        {
            self.receive_vibration(&mut state, v);
        }
        if self.mob_entity.persistence_required.load(Ordering::Relaxed)
            || entity.custom_name.load().is_some()
        {
            self.reset_dig();
        }
        if state.ticks % 20 == 0 {
            state.anger.retain_mut(|(uuid, anger)| {
                *anger -= 1;
                *anger > 0
                    && world
                        .get_entity_by_uuid(*uuid)
                        .is_none_or(|e| self.valid(e.as_ref()))
            });
        }
        if state.ticks % 120 == 0 {
            Self::apply_darkness_around(&world, entity.pos.load(), 20.0);
        }
        if let Some((_, remaining)) = &mut state.disturbance {
            *remaining -= 1;
            if *remaining <= 0 {
                state.disturbance = None;
            }
        }
        if !self.memory_present(types::TOUCH_COOLDOWN)
            && let Some(target) = world
                .get_all_at_box(&entity.bounding_box.load().expand(0.1, 0.0, 0.1))
                .into_iter()
                .find(|e| self.valid(e.as_ref()))
        {
            self.increase_anger(&mut state, target.as_ref(), 35);
            self.remember(types::TOUCH_COOLDOWN, 20);
            state.disturbance = Some((target.get_entity().pos.load(), 100));
        }
        let mut target = self.mob_entity.get_target();
        if target.as_ref().is_some_and(|t| {
            !self.valid(t.as_ref())
                || !state
                    .anger
                    .iter()
                    .any(|(id, anger)| *id == t.get_entity().entity_uuid && *anger >= ANGRY)
        }) {
            self.set_attack_target(None);
            self.stop_moving();
            self.reset_dig();
            target = None;
        }
        let best = state
            .anger
            .iter()
            .filter(|(_, anger)| *anger >= ANGRY)
            .filter_map(|(id, anger)| {
                world
                    .get_entity_by_uuid(*id)
                    .filter(|e| self.valid(e.as_ref()))
                    .map(|e| (*anger, e))
            })
            .max_by_key(|(anger, e)| (e.get_player().is_some(), *anger));
        let active_anger = target
            .as_ref()
            .and_then(|t| {
                state
                    .anger
                    .iter()
                    .find(|(id, _)| *id == t.get_entity().entity_uuid)
            })
            .map_or_else(
                || state.anger.iter().map(|(_, a)| *a).max().unwrap_or(0),
                |(_, a)| *a,
            );
        entity.set_synced_data(
            pumpkin_data::tracked_data::warden::CLIENT_ANGER_LEVEL,
            VarInt(active_anger),
        );
        if matches!(state.phase, Phase::Idle | Phase::Sniffing(_))
            && target.is_none()
            && let Some((_, suspect)) = best
        {
            self.stop_moving();
            self.increase_anger(&mut state, suspect.as_ref(), 20);
            state.phase = Phase::Roaring(suspect.get_entity().entity_uuid, 84);
            entity.set_pose(EntityPose::Roaring);
        }
        match state.phase {
            Phase::Emerging(remaining) => {
                self.stop_moving();
                state.phase = if remaining <= 1 {
                    entity.set_pose(EntityPose::Standing);
                    Phase::Idle
                } else {
                    Phase::Emerging(remaining - 1)
                };
            }
            Phase::Digging(remaining) => {
                self.stop_moving();
                if remaining <= 1 {
                    entity.remove();
                } else {
                    state.phase = Phase::Digging(remaining - 1);
                }
            }
            Phase::Roaring(uuid, remaining) => {
                self.stop_moving();
                if let Some(suspect) = world.get_entity_by_uuid(uuid) {
                    self.mob_entity
                        .look_control
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .look_at_entity_with_range(&suspect, 45.0, 90.0);
                }
                if remaining == 59 {
                    entity.play_sound(Sound::EntityWardenRoar);
                }
                if remaining <= 1 {
                    if let Some(suspect) = world
                        .get_entity_by_uuid(uuid)
                        .filter(|e| self.valid(e.as_ref()))
                    {
                        self.set_attack_target(Some(suspect));
                    }
                    state.phase = Phase::Idle;
                    entity.set_pose(EntityPose::Standing);
                } else {
                    state.phase = Phase::Roaring(uuid, remaining - 1);
                }
            }
            Phase::Sonic(uuid, remaining) => {
                self.stop_moving();
                if let Some(suspect) = world.get_entity_by_uuid(uuid) {
                    self.mob_entity
                        .look_control
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .look_at_entity_with_range(&suspect, 45.0, 90.0);
                    if remaining == 26
                        && target
                            .as_ref()
                            .is_some_and(|current| current.get_entity().entity_uuid == uuid)
                    {
                        self.sonic_boom(suspect.as_ref());
                    }
                }
                if remaining <= 1 {
                    self.remember(types::SONIC_BOOM_COOLDOWN, 40);
                    state.phase = Phase::Idle;
                } else {
                    state.phase = Phase::Sonic(uuid, remaining - 1);
                }
            }
            Phase::Sniffing(remaining) => {
                self.stop_moving();
                if remaining <= 1 {
                    if let Some(suspect) = self.nearest_suspect() {
                        let delta = suspect.get_entity().pos.load() - entity.pos.load();
                        if delta.x * delta.x + delta.z * delta.z < 36.0 && delta.y.abs() < 20.0 {
                            self.increase_anger(&mut state, suspect.as_ref(), 35);
                        }
                        state.disturbance = Some((suspect.get_entity().pos.load(), 100));
                        self.reset_dig();
                    }
                    entity.set_pose(EntityPose::Standing);
                    state.phase = Phase::Idle;
                } else {
                    state.phase = Phase::Sniffing(remaining - 1);
                }
            }
            Phase::Idle => {
                if let Some(target) = target {
                    self.reset_dig();
                    state.disturbance = None;
                    self.mob_entity
                        .look_control
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .look_at_entity_with_range(&target, 45.0, 90.0);
                    if self.mob_entity.is_in_attack_range(target.as_ref())
                        && self.has_line_of_sight(target.get_entity())
                    {
                        self.stop_moving();
                        if state.attack_cooldown == 0 {
                            self.mob_entity.try_attack(self, target.as_ref());
                            state.attack_cooldown = 18;
                            self.remember(types::SONIC_BOOM_COOLDOWN, 40);
                            world.send_entity_status(entity, EntityStatus::StartAttacking, None);
                            entity.play_sound(Sound::EntityWardenAttackImpact);
                        }
                    } else if !self.memory_present(types::SONIC_BOOM_COOLDOWN)
                        && self.sonic_in_range(target.as_ref())
                    {
                        self.stop_moving();
                        state.phase = Phase::Sonic(target.get_entity().entity_uuid, 60);
                        state.attack_cooldown = 60;
                        world.send_entity_status(entity, EntityStatus::SonicCharge, None);
                        entity.play_sound(Sound::EntityWardenSonicCharge);
                    } else if state.ticks % 10 == 0 {
                        self.navigate(target.get_entity().pos.load(), 1.2);
                    }
                } else if !self.memory_present(types::DIG_COOLDOWN)
                    && (entity.on_ground.load(Ordering::Relaxed)
                        || entity.touching_water.load(Ordering::Relaxed))
                {
                    self.stop_moving();
                    if entity.on_ground.load(Ordering::Relaxed) {
                        state.phase = Phase::Digging(100);
                        entity.set_pose(EntityPose::Digging);
                        entity.play_sound(Sound::EntityWardenDig);
                    } else {
                        entity.play_sound(Sound::EntityWardenAgitated);
                        entity.remove();
                    }
                } else if let Some((position, _)) = state.disturbance {
                    if position.squared_distance_to_vec(&entity.pos.load()) < 4.0 {
                        self.stop_moving();
                    } else if state.ticks % 10 == 0 {
                        self.navigate(position, 0.7);
                    }
                } else if !self.memory_present(types::SNIFF_COOLDOWN)
                    && self.nearest_suspect().is_some()
                {
                    self.stop_moving();
                    state.phase = Phase::Sniffing(84);
                    entity.set_pose(EntityPose::Sniffing);
                    entity.play_sound(Sound::EntityWardenSniff);
                    self.remember(types::SNIFF_COOLDOWN, rand::random_range(100..=200));
                } else if state.ticks % 120 == 0
                    && self.is_navigator_idle()
                    && let Some(position) = land_random_pos::get_pos(self, 10, 7)
                {
                    self.navigate(position, 0.5);
                }
            }
        }
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
    }
    pub fn apply_darkness_around(world: &World, position: Vector3<f64>, radius: f64) {
        for player in world.get_nearby_players(position, radius) {
            if !matches!(
                player.gamemode.load(),
                GameMode::Survival | GameMode::Adventure
            ) {
                continue;
            }
            if player
                .living_entity
                .get_effect(&StatusEffect::DARKNESS)
                .is_some_and(|effect| effect.duration < 0 || effect.duration >= 200)
            {
                continue;
            }
            let darkness = Effect {
                effect_type: &StatusEffect::DARKNESS,
                duration: 260,
                amplifier: 0,
                ambient: false,
                show_particles: false,
                show_icon: false,
                blend: true,
            };
            player.send_effect(&darkness);
            player.living_entity.add_effect(darkness);
        }
    }
}
struct WardenBehavior {
    warden: Weak<WardenEntity>,
}
impl Goal for WardenBehavior {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity().living_entity.is_alive()
    }
    fn tick(&mut self, _mob: &dyn Mob) {
        if let Some(warden) = self.warden.upgrade() {
            warden.tick_ai();
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
        Controls::MOVE | Controls::LOOK | Controls::TARGET
    }
}
impl Mob for WardenEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
    fn remove_when_far_away(&self, _distance_sq: f64) -> bool {
        false
    }
    fn check_spawn_obstruction(&self, world: &World) -> bool {
        let bounds = self.get_entity().bounding_box.load();
        !world.contains_any_liquid(bounds)
            && world.get_entities_at_box(&bounds).is_empty()
            && world.is_space_empty(bounds)
    }
    fn make_brain(&self, packed: &PackedMemories) -> Brain {
        let mut brain = Brain::default();
        for key in [
            types::DIG_COOLDOWN,
            types::SONIC_BOOM_COOLDOWN,
            types::SNIFF_COOLDOWN,
            types::VIBRATION_COOLDOWN,
            types::TOUCH_COOLDOWN,
            types::RECENT_PROJECTILE,
            types::IS_EMERGING,
        ] {
            brain.register_memory(key.id());
        }
        brain.register_memory(types::ATTACK_TARGET.id());
        brain.register_memory(types::ROAR_TARGET.id());
        brain.set_with_expiry(types::DIG_COOLDOWN, (), 1200);
        brain.load_packed(packed);
        brain
    }
    fn pre_damage(&self, damage_type: DamageType, _source: Option<&dyn EntityBase>) -> bool {
        !matches!(
            self.get_entity().pose.load(),
            EntityPose::Emerging | EntityPose::Digging
        ) || damage_type.has_tag(&tag::DamageType::MINECRAFT_BYPASSES_INVULNERABILITY)
    }
    fn on_damage(&self, damage_type: DamageType, source: Option<&dyn EntityBase>) {
        if self.mob_entity.is_no_ai() {
            return;
        }
        if let Some(source) = source
            && self.valid(source)
        {
            let immediate = !damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_PROJECTILE)
                || source
                    .get_entity()
                    .pos
                    .load()
                    .squared_distance_to_vec(&self.get_entity().pos.load())
                    < 25.0;
            self.queue(Stimulus::Damage(source.get_entity().entity_uuid, immediate));
        }
    }
    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut anger = NbtCompound::new();
        anger.put(
            "suspects",
            NbtTag::List(
                state
                    .anger
                    .iter()
                    .map(|(uuid, value)| {
                        let mut suspect = NbtCompound::new();
                        suspect.put_uuid("uuid", *uuid);
                        suspect.put_int("anger", *value);
                        NbtTag::Compound(suspect)
                    })
                    .collect(),
            ),
        );
        nbt.put_compound("anger", anger);
    }
    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        let mut state = WardenState::default();
        if let Some(suspects) = nbt
            .get_compound("anger")
            .and_then(|anger| anger.get_list("suspects"))
        {
            for suspect in suspects {
                if let NbtTag::Compound(suspect) = suspect
                    && let (Some(uuid), Some(anger)) =
                        (suspect.get_uuid("uuid"), suspect.get_int("anger"))
                {
                    if !state.anger.iter().any(|(id, _)| *id == uuid) {
                        state.anger.push((uuid, anger.clamp(0, 150)));
                    }
                }
            }
        }
        if let Some(ticks) = self
            .mob_entity
            .brain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .time_until_expiry(types::IS_EMERGING.id())
        {
            state.phase = Phase::Emerging(ticks.clamp(1, 134) as i32);
            self.get_entity().set_pose(EntityPose::Emerging);
        }
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
        self.stimuli
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}
