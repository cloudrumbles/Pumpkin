//! Water navigation and swimming goals shared by aquatic mobs.

use std::sync::{Arc, Weak, atomic::Ordering};

use pumpkin_data::{
    entity::EntityType,
    tag::{self, Taggable},
};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};

use super::{Controls, Goal};
use crate::entity::{
    EntityBase,
    ai::pathfinder::{Navigator, NavigatorGoal},
    mob::{Mob, MobEntity},
};

pub fn configure_swimming(mob: &MobEntity) {
    *mob.navigator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Navigator::water_bound(false);
}

pub fn aquatic_gravity(mob: &MobEntity) -> f64 {
    if mob
        .living_entity
        .entity
        .touching_water
        .load(Ordering::Relaxed)
    {
        0.0
    } else {
        mob.living_entity.get_gravity()
    }
}

/// Picks submerged destinations and follows a bounded school when requested.
/// Leaders always have a lower entity ID, so two fish cannot follow each other.
pub struct RandomSwimGoal {
    speed: f64,
    interval: i64,
    next_attempt: i64,
    expires: i64,
    destination: Option<Vector3<f64>>,
    leader: Option<Weak<dyn EntityBase>>,
    school_size: usize,
    avoid_players: bool,
    next_repath: i64,
}

impl RandomSwimGoal {
    pub const fn new(speed: f64, interval: i64) -> Self {
        Self {
            speed,
            interval,
            next_attempt: 0,
            expires: 0,
            destination: None,
            leader: None,
            school_size: 0,
            avoid_players: false,
            next_repath: 0,
        }
    }

    pub fn fish(school_size: usize) -> Self {
        Self {
            school_size,
            avoid_players: true,
            ..Self::new(1.0, 40)
        }
    }

    fn underwater(mob: &dyn Mob, point: Vector3<f64>) -> bool {
        mob.get_entity()
            .world
            .load()
            .get_fluid(&BlockPos::floored_v(point))
            .has_tag(&tag::Fluid::MINECRAFT_WATER)
    }

    fn navigate(&self, mob: &dyn Mob, destination: Vector3<f64>, speed: f64) {
        let entity = mob.get_entity();
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal {
                current_progress: entity.pos.load(),
                destination,
                speed,
            });
    }
}

impl Goal for RandomSwimGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        if !entity.touching_water.load(Ordering::Relaxed)
            || mob.is_sitting()
            || mob.get_mob_entity().get_target().is_some()
            || !entity
                .passengers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        {
            return false;
        }
        let world = entity.world.load();
        let now = world.get_world_age();
        if now < self.next_attempt {
            return false;
        }
        self.next_attempt = now + self.interval;
        let pos = entity.pos.load();
        self.destination = None;
        self.leader = None;

        if self.avoid_players {
            if let Some(player) = world.get_nearest_player(pos, 8.0, |p| !p.is_spectator()) {
                let away = pos - player.get_entity().pos.load();
                let len = away.length();
                if len > 0.001 {
                    let escape = pos + away * (5.0 / len);
                    if Self::underwater(mob, escape) {
                        self.destination = Some(escape);
                    }
                }
            }
        }
        if self.destination.is_none() && self.school_size > 0 {
            let bounds = entity.bounding_box.load().expand(8.0, 8.0, 8.0);
            let mut fish: Vec<_> = world
                .get_entities_at_box(&bounds)
                .into_iter()
                .filter(|other| {
                    let e = other.get_entity();
                    e.entity_type == entity.entity_type
                        && e.is_alive()
                        && e.entity_id < entity.entity_id
                })
                .collect();
            fish.sort_unstable_by_key(|other| other.get_entity().entity_id);
            if fish.len() < self.school_size {
                if let Some(leader) = fish.first() {
                    let destination = leader.get_entity().pos.load();
                    if pos.squared_distance_to_vec(&destination) > 4.0 {
                        self.destination = Some(destination);
                        self.leader = Some(Arc::downgrade(leader));
                    }
                }
            }
        }
        if self.destination.is_none() {
            for _ in 0..10 {
                let candidate = pos
                    + Vector3::new(
                        rand::random_range(-10.0..=10.0),
                        rand::random_range(-5.0..=5.0),
                        rand::random_range(-10.0..=10.0),
                    );
                if Self::underwater(mob, candidate) {
                    self.destination = Some(candidate);
                    break;
                }
            }
        }
        self.destination.is_some()
    }

    fn start(&mut self, mob: &dyn Mob) {
        let now = mob.get_entity().world.load().get_world_age();
        self.expires = now + 200;
        self.next_repath = now + 10;
        if let Some(destination) = self.destination {
            self.navigate(mob, destination, self.speed);
        }
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.get_entity().touching_water.load(Ordering::Relaxed)
            && mob.get_entity().world.load().get_world_age() < self.expires
            && mob.get_mob_entity().get_target().is_none()
            && !mob.is_navigator_idle()
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let now = mob.get_entity().world.load().get_world_age();
        if now < self.next_repath {
            return;
        }
        self.next_repath = now + 10;
        if let Some(leader) = self.leader.as_ref().and_then(Weak::upgrade) {
            let destination = leader.get_entity().pos.load();
            if !leader.get_entity().is_alive() || !Self::underwater(mob, destination) {
                self.expires = now;
            } else if mob
                .get_entity()
                .pos
                .load()
                .squared_distance_to_vec(&destination)
                > 4.0
            {
                self.navigate(mob, destination, self.speed);
            }
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
        Controls::MOVE
    }
}

/// Fish flop on land rather than running a surface-seeking float goal.
pub fn flop(mob: &dyn Mob) {
    flop_with_strength(mob, 0.4);
}

pub fn flop_with_strength(mob: &dyn Mob, strength: f64) {
    let entity = mob.get_entity();
    if !entity.touching_water.load(Ordering::Relaxed) && entity.on_ground.load(Ordering::Relaxed) {
        let old = entity.velocity.load();
        entity.set_velocity(
            old + Vector3::new(
                rand::random_range(-0.05..0.05),
                strength,
                rand::random_range(-0.05..0.05),
            ),
        );
        entity.on_ground.store(false, Ordering::Relaxed);
        entity.yaw.store(rand::random_range(0.0..360.0));
    }
}

pub const fn school_size(entity_type: &'static EntityType) -> usize {
    if entity_type.id == EntityType::COD.id {
        9
    } else if entity_type.id == EntityType::SALMON.id {
        5
    } else if entity_type.id == EntityType::TROPICAL_FISH.id {
        8
    } else {
        0
    }
}
