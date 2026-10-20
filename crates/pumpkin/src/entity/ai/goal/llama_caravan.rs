//! Vanilla llama caravans: only an un-leashed llama may join a chain which is
//! eventually led by a leashed llama. Each llama has at most one head and tail.
use std::sync::{Arc, Mutex, Weak};

use pumpkin_data::entity::EntityType;

use super::{Controls, Goal};
use crate::entity::passive::{llama::LlamaEntity, trader_llama::TraderLlamaEntity};
use crate::entity::{EntityBase, ai::pathfinder::NavigatorGoal, mob::Mob};

#[derive(Default)]
pub struct CaravanState {
    head: Mutex<Option<Weak<dyn EntityBase>>>,
    tail: Mutex<Option<Weak<dyn EntityBase>>>,
}

impl CaravanState {
    fn head(&self) -> Option<Arc<dyn EntityBase>> {
        self.head
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade)
    }
    fn tail(&self) -> Option<Arc<dyn EntityBase>> {
        self.tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Weak::upgrade)
    }
    fn set_head(&self, head: Option<&Arc<dyn EntityBase>>) {
        *self
            .head
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = head.map(Arc::downgrade);
    }
    fn set_tail(&self, tail: Option<&Arc<dyn EntityBase>>) {
        *self
            .tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = tail.map(Arc::downgrade);
    }
}

fn llama_state(entity: &dyn EntityBase) -> Option<&CaravanState> {
    if let Some(llama) = entity.cast_any().downcast_ref::<LlamaEntity>() {
        Some(&llama.caravan)
    } else {
        entity
            .cast_any()
            .downcast_ref::<TraderLlamaEntity>()
            .map(|llama| &llama.caravan)
    }
}

/// The head of a valid chain must itself be leashed, or linked through at most
/// eight llamas to a leashed leader. This also detects any accidental cycle.
fn chain_has_leader(mut head: Arc<dyn EntityBase>) -> bool {
    let mut seen = std::collections::HashSet::new();
    for _ in 0..8 {
        let entity = head.get_entity();
        if !seen.insert(entity.entity_id) || !entity.is_alive() {
            return false;
        }
        if entity.is_leashed() {
            return true;
        }
        let Some(next) = llama_state(head.as_ref()).and_then(CaravanState::head) else {
            return false;
        };
        head = next;
    }
    false
}

pub struct LlamaFollowCaravanGoal {
    head: Option<Arc<dyn EntityBase>>,
    speed: f64,
    far_ticks: i32,
    recalc: i32,
}

impl LlamaFollowCaravanGoal {
    pub const fn new() -> Self {
        Self {
            head: None,
            speed: 2.1,
            far_ticks: 0,
            recalc: 0,
        }
    }
}

impl Goal for LlamaFollowCaravanGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let entity = mob.get_entity();
        if entity.is_leashed() || llama_state(mob).is_none() {
            return false;
        }
        if llama_state(mob).is_some_and(|state| state.head().is_some()) {
            return false;
        }
        let candidates = entity
            .world
            .load()
            .get_nearby_entities(entity.pos.load(), 9.0);
        let mut selected: Option<(f64, Arc<dyn EntityBase>)> = None;
        for candidate in candidates.values() {
            let other = candidate.get_entity();
            if other.entity_id == entity.entity_id
                || (other.entity_type != &EntityType::LLAMA
                    && other.entity_type != &EntityType::TRADER_LLAMA)
                || !other.is_alive()
            {
                continue;
            }
            let Some(state) = llama_state(candidate.as_ref()) else {
                continue;
            };
            if state.tail().is_some() || !chain_has_leader(candidate.clone()) {
                continue;
            }
            let distance = entity.pos.load().squared_distance_to_vec(&other.pos.load());
            if distance < 4.0 || selected.as_ref().is_some_and(|(best, _)| distance >= *best) {
                continue;
            }
            selected = Some((distance, candidate.clone()));
        }
        self.head = selected.map(|(_, head)| head);
        self.head.is_some()
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(head) = self.head.as_ref() else {
            return;
        };
        let Some(state) = llama_state(mob) else {
            return;
        };
        let entity = mob.get_entity();
        let Some(self_arc) = entity.world.load().get_entity_by_id(entity.entity_id) else {
            return;
        };
        state.set_head(Some(head));
        if let Some(head_state) = llama_state(head.as_ref()) {
            head_state.set_tail(Some(&self_arc));
        }
        self.speed = 2.1;
        self.far_ticks = 0;
        self.recalc = 0;
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(head) = &self.head else {
            return false;
        };
        let Some(state) = llama_state(mob) else {
            return false;
        };
        if mob.get_entity().is_leashed()
            || !head.get_entity().is_alive()
            || state
                .head()
                .as_ref()
                .is_none_or(|current| current.get_entity().entity_id != head.get_entity().entity_id)
            || !chain_has_leader(head.clone())
        {
            return false;
        }
        let dist = mob
            .get_entity()
            .pos
            .load()
            .squared_distance_to_vec(&head.get_entity().pos.load());
        if dist > 26.0 * 26.0 {
            if self.speed <= 3.0 {
                self.speed *= 1.2;
                self.far_ticks = 40;
            } else if self.far_ticks <= 0 {
                return false;
            }
            self.far_ticks -= 1;
        }
        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        self.recalc -= 1;
        if self.recalc > 0 {
            return;
        }
        self.recalc = 10;
        let Some(head) = self.head.as_ref() else {
            return;
        };
        let pos = mob.get_entity().pos.load();
        let other_pos = head.get_entity().pos.load();
        let offset = other_pos - pos;
        let distance = offset.length();
        if distance <= 2.0 {
            return;
        }
        // Stop two blocks behind the head rather than walking into it.
        let destination = pos + offset * ((distance - 2.0) / distance);
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_progress(NavigatorGoal::new(pos, destination, self.speed));
    }

    fn stop(&mut self, mob: &dyn Mob) {
        if let Some(state) = llama_state(mob) {
            state.set_head(None);
        }
        if let Some(head) = self.head.take() {
            if let Some(state) = llama_state(head.as_ref()) {
                // Only detach our own link (another llama may have joined).
                if state
                    .tail()
                    .is_some_and(|tail| tail.get_entity().entity_id == mob.get_entity().entity_id)
                {
                    state.set_tail(None);
                }
            }
        }
        self.speed = 2.1;
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// A trader llama only attacks whoever most recently hurt the wandering trader
/// to which it is physically leashed. Normal llamas do not receive this goal.
pub struct TraderLlamaDefendGoal {
    timestamp: i32,
    target: Option<Arc<dyn EntityBase>>,
}

impl TraderLlamaDefendGoal {
    pub const fn new() -> Self {
        Self {
            timestamp: 0,
            target: None,
        }
    }
}

impl Goal for TraderLlamaDefendGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let guard = mob
            .get_entity()
            .leashed_to
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(holder) = guard
            .as_ref()
            .filter(|e| e.get_entity().entity_type == &EntityType::WANDERING_TRADER)
        else {
            return false;
        };
        let Some(living) = holder.get_living_entity() else {
            return false;
        };
        let stamp = living
            .last_attacked_time
            .load(std::sync::atomic::Ordering::Relaxed);
        if stamp == self.timestamp {
            return false;
        }
        let attacker_id = living
            .last_attacker_id
            .load(std::sync::atomic::Ordering::Relaxed);
        let Some(attacker) = mob.get_entity().world.load().get_entity_by_id(attacker_id) else {
            return false;
        };
        if !super::is_valid_attack_target(mob, attacker.as_ref()) {
            return false;
        }
        self.target = Some(attacker);
        true
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.timestamp = mob
            .get_entity()
            .leashed_to
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|holder| holder.get_living_entity())
            .map_or(self.timestamp, |living| {
                living
                    .last_attacked_time
                    .load(std::sync::atomic::Ordering::Relaxed)
            });
        mob.set_mob_target(self.target.clone());
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.target
            .as_ref()
            .is_some_and(|t| super::is_valid_attack_target(mob, t.as_ref()))
    }
    fn stop(&mut self, _mob: &dyn Mob) {
        self.target = None;
    }
    fn controls(&self) -> Controls {
        Controls::TARGET
    }
}
