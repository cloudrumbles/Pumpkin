//! Java 26.3 NeutralMob anger, including absolute game-time persistence.

use crossbeam::atomic::AtomicCell;
use pumpkin_data::{attributes::Attributes, entity::EntityType, tracked_data};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::{Difficulty, math::boundingbox::BoundingBox};
use rand::RngExt;
use std::sync::{
    Arc,
    atomic::{AtomicI32, AtomicI64, Ordering::Relaxed},
};
use uuid::Uuid;

use super::{
    goal::{Controls, Goal, is_valid_attack_target, track_target::TrackTargetGoal},
    target_predicate::TargetPredicate,
};
use crate::entity::{EntityBase, mob::Mob};

pub fn is_neutral(kind: &EntityType) -> bool {
    [
        &EntityType::BEE,
        &EntityType::ENDERMAN,
        &EntityType::IRON_GOLEM,
        &EntityType::POLAR_BEAR,
        &EntityType::WOLF,
        &EntityType::ZOMBIFIED_PIGLIN,
    ]
    .contains(&kind)
}

pub struct PersistentAnger {
    pub end_time: AtomicI64,
    pub target: AtomicCell<Option<Uuid>>,
}

impl Default for PersistentAnger {
    fn default() -> Self {
        Self {
            end_time: AtomicI64::new(-1),
            target: AtomicCell::new(None),
        }
    }
}

impl PersistentAnger {
    pub fn is_angry(&self, mob: &dyn Mob) -> bool {
        self.end_time.load(Relaxed) > mob.get_entity().world.load().get_world_age()
    }

    pub fn refresh(&self, mob: &dyn Mob) {
        self.end_time.store(
            mob.get_entity().world.load().get_world_age()
                + i64::from(rand::rng().random_range(400..=780)),
            Relaxed,
        );
    }

    pub fn stop(&self, mob: &dyn Mob) {
        self.target.store(None);
        self.end_time.store(-1, Relaxed);
        mob.get_mob_entity()
            .living_entity
            .last_attacker_id
            .store(0, Relaxed);
        mob.set_mob_target(None);
        self.sync(mob);
    }

    pub fn is_angry_at(&self, mob: &dyn Mob, target: &dyn EntityBase) -> bool {
        if !is_valid_attack_target(mob, target) {
            return false;
        }
        let world = mob.get_entity().world.load();
        if target.get_player().is_some()
            && world.level_info.load().difficulty != Difficulty::Peaceful
            && world.level_info.load().game_rules.universal_anger
            && self.is_angry(mob)
            && self.target.load().is_none()
        {
            return true;
        }
        self.target.load() == Some(target.get_entity().entity_uuid)
    }

    pub fn update(&self, mob: &dyn Mob) {
        let world = mob.get_entity().world.load();
        let previous = mob.get_mob_entity().get_target();
        if previous.as_ref().is_some_and(|target| {
            target.get_mob().is_some()
                && target
                    .get_living_entity()
                    .is_none_or(|living| !living.is_alive())
                && self.target.load() == Some(target.get_entity().entity_uuid)
        }) {
            self.stop(mob);
            return;
        }
        let target = previous.filter(|target| is_valid_attack_target(mob, target.as_ref()));
        if let Some(target) = target {
            let changed = self.target.load() != Some(target.get_entity().entity_uuid);
            if changed {
                self.target.store(Some(target.get_entity().entity_uuid));
            }
            // Bees keep a fixed timer; the other neutral mobs renew it while fighting.
            if changed || mob.get_entity().entity_type != &EntityType::BEE {
                self.refresh(mob);
            }
        }
        if self.target.load().is_some() && !self.is_angry(mob) {
            self.stop(mob);
            return;
        }
        if let Some(target) = self
            .target
            .load()
            .and_then(|id| world.get_entity_by_uuid(id))
            && let Some(player) = target.get_player()
        {
            let rules = world.level_info.load();
            if player.is_creative()
                || player.is_spectator()
                || rules.difficulty == Difficulty::Peaceful
                || (rules.game_rules.forgive_dead_players && !player.living_entity.is_alive())
            {
                self.stop(mob);
                return;
            }
        }
        self.sync(mob);
    }

    pub fn sync(&self, mob: &dyn Mob) {
        let entity = mob.get_entity();
        if entity.entity_type == &EntityType::BEE {
            entity.set_synced_data(
                tracked_data::bee::DATA_ANGER_END_TIME,
                self.end_time.load(Relaxed),
            );
        } else if entity.entity_type == &EntityType::WOLF {
            entity.set_synced_data(
                tracked_data::wolf::DATA_ANGER_END_TIME,
                self.end_time.load(Relaxed),
            );
        }
    }

    pub fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_long("anger_end_time", self.end_time.load(Relaxed));
        if let Some(target) = self.target.load() {
            nbt.put_uuid("angry_at", target);
        }
    }

    pub fn read_nbt(&self, mob: &dyn Mob, nbt: &NbtCompound) {
        let end = nbt.get_long("anger_end_time").unwrap_or_else(|| {
            nbt.get_int("AngerTime").map_or(-1, |ticks| {
                mob.get_entity().world.load().get_world_age() + i64::from(ticks)
            })
        });
        self.end_time.store(end, Relaxed);
        let id = nbt.get_uuid("angry_at").or_else(|| nbt.get_uuid("AngryAt"));
        self.target.store(id);
        mob.set_mob_target(id.and_then(|id| mob.get_entity().world.load().get_entity_by_uuid(id)));
        self.sync(mob);
    }
}

pub struct AngryPlayerGoal {
    target: Option<Arc<dyn EntityBase>>,
    tracking: TrackTargetGoal,
}

impl Default for AngryPlayerGoal {
    fn default() -> Self {
        Self {
            target: None,
            tracking: TrackTargetGoal::with_default(true),
        }
    }
}

impl Goal for AngryPlayerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if rand::rng().random_range(0..5) != 0 || !mob.can_target_while_angry() {
            return false;
        }
        let Some(anger) = mob.as_neutral() else {
            return false;
        };
        let world = mob.get_entity().world.load();
        let range = mob
            .get_mob_entity()
            .living_entity
            .get_attribute_value(&Attributes::FOLLOW_RANGE);
        let conditions = TargetPredicate::create_attackable().set_base_max_distance(range);
        self.target = world
            .get_nearest_player(mob.get_entity().get_eye_pos(), range, |player| {
                anger.is_angry_at(mob, player.as_ref())
                    && conditions.test(&world, Some(mob), player.as_ref())
            })
            .map(|player| player as Arc<dyn EntityBase>);
        self.target.is_some()
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.can_target_while_angry() && self.tracking.should_continue(mob)
    }
    fn start(&mut self, mob: &dyn Mob) {
        mob.set_mob_target(self.target.clone());
        self.tracking.start(mob);
    }
    fn stop(&mut self, mob: &dyn Mob) {
        self.target = None;
        self.tracking.stop(mob);
    }
    fn controls(&self) -> Controls {
        Controls::TARGET
    }
}

#[derive(Default)]
pub struct ResetUniversalAngerGoal {
    last_hurt: AtomicI32,
}

impl Goal for ResetUniversalAngerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let living = &mob.get_mob_entity().living_entity;
        let world = mob.get_entity().world.load();
        world.level_info.load().game_rules.universal_anger
            && living.last_attacked_time.load(Relaxed) > self.last_hurt.load(Relaxed)
            && world
                .get_entity_by_id(living.last_attacker_id.load(Relaxed))
                .is_some_and(|attacker| attacker.get_player().is_some())
    }
    fn start(&mut self, mob: &dyn Mob) {
        self.last_hurt.store(
            mob.get_mob_entity()
                .living_entity
                .last_attacked_time
                .load(Relaxed),
            Relaxed,
        );
        let Some(anger) = mob.as_neutral() else {
            return;
        };
        anger.stop(mob);
        anger.refresh(mob);
        let entity = mob.get_entity();
        if entity.entity_type != &EntityType::BEE
            && entity.entity_type != &EntityType::ZOMBIFIED_PIGLIN
        {
            return;
        }
        let distance = mob
            .get_mob_entity()
            .living_entity
            .get_attribute_value(&Attributes::FOLLOW_RANGE);
        let pos = entity.pos.load();
        let bbox =
            BoundingBox::new(pos, pos.add_raw(1.0, 1.0, 1.0)).expand(distance, 10.0, distance);
        for other in entity.world.load().get_entities_at_box(&bbox) {
            if other.get_entity().entity_id == entity.entity_id
                || other.get_entity().entity_type != entity.entity_type
                || other.is_spectator()
            {
                continue;
            }
            if let Some(other) = other.get_mob()
                && let Some(anger) = other.as_neutral()
            {
                anger.stop(other);
                anger.refresh(other);
            }
        }
    }
}
