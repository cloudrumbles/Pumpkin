use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::{
    attributes::Attributes,
    sound::{Sound, SoundCategory},
};
use pumpkin_util::math::boundingbox::BoundingBox;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        destroy_egg::DestroyEggGoal, look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal, revenge::RevengeGoal, swim::SwimGoal,
        wander_around::WanderAroundGoal, zombie_attack::ZombieAttackGoal,
    },
    mob::{Mob, MobEntity, equipment::RegionalDifficulty},
};
use crate::world::World;

pub struct ZombifiedPiglinEntity {
    pub mob_entity: MobEntity,
    play_first_anger_sound_in: AtomicI32,
    speed_boosted: AtomicBool,
    ticks_until_next_alert: AtomicI32,
}

impl ZombifiedPiglinEntity {
    pub const XP_REWARD: u32 = 5;

    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let piglin = Self {
            mob_entity,
            play_first_anger_sound_in: AtomicI32::new(0),
            speed_boosted: AtomicBool::new(false),
            ticks_until_next_alert: AtomicI32::new(0),
        };
        let mob_arc = Arc::new(piglin);
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
            goal_selector.add_goal(2, ZombieAttackGoal::new(1.0, false));
            goal_selector.add_goal(4, DestroyEggGoal::new(1.0, 3));
            goal_selector.add_goal(7, Box::new(WanderAroundGoal::avoiding_water(1.0, 0.001)));
            goal_selector.add_goal(
                8,
                LookAtEntityGoal::with_default(mob_weak.clone(), &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(8, Box::new(RandomLookAroundGoal::default()));

            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            target_selector.add_goal(1, Box::new(RevengeGoal::new(true).alerting_others()));
        };

        mob_arc
            .mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_pathfinding_malus(crate::entity::ai::pathfinder::node::PathType::Lava, 8.0);
        mob_arc
    }

    pub fn is_angry(&self) -> bool {
        self.mob_entity.anger.is_angry(self)
    }
}

impl Mob for ZombifiedPiglinEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        _difficulty: &RegionalDifficulty,
    ) {
        let living = &self.mob_entity.living_entity;
        let mut equipment = living
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let weapon = if rand::random_range(0..20) == 0 {
            &Item::GOLDEN_SPEAR
        } else {
            &Item::GOLDEN_SWORD
        };
        equipment.put(&EquipmentSlot::MAIN_HAND, ItemStack::new(1, weapon));
    }

    fn on_target_changed(&self, had_target: bool, has_target: bool) {
        if !had_target && has_target {
            self.play_first_anger_sound_in
                .store(rand::random_range(0..=20), Ordering::Relaxed);
            self.ticks_until_next_alert
                .store(rand::random_range(80..=120), Ordering::Relaxed);
        }
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let living = &self.mob_entity.living_entity;
        let entity = &living.entity;
        let angry = self.is_angry();
        if self.speed_boosted.swap(angry, Ordering::Relaxed) != angry {
            living.update_attribute(&Attributes::MOVEMENT_SPEED, |speed| {
                if angry {
                    speed.add_or_replace_modifier(crate::entity::attributes::Modifier {
                        id: "minecraft:attacking".into(),
                        amount: 0.05,
                        operation: crate::entity::attributes::ModifierOperation::Add,
                    });
                } else {
                    speed.remove_modifier("minecraft:attacking");
                }
            });
        }
        if angry
            && self.play_first_anger_sound_in.load(Ordering::Relaxed) > 0
            && self
                .play_first_anger_sound_in
                .fetch_sub(1, Ordering::Relaxed)
                == 1
        {
            entity.world.load().play_sound_fine(
                Sound::EntityZombifiedPiglinAngry,
                SoundCategory::Hostile,
                &entity.pos.load(),
                2.0,
                living.get_pitch() * 1.8,
            );
        }
        if let Some(target) = self.mob_entity.get_target() {
            if self.ticks_until_next_alert.load(Ordering::Relaxed) > 0 {
                self.ticks_until_next_alert.fetch_sub(1, Ordering::Relaxed);
                return;
            }
            self.ticks_until_next_alert
                .store(rand::random_range(80..=120), Ordering::Relaxed);
            if !self.has_line_of_sight(target.get_entity()) {
                return;
            }
            let within = living.get_attribute_value(&Attributes::FOLLOW_RANGE);
            let pos = entity.pos.load();
            let bounds =
                BoundingBox::new(pos, pos.add_raw(1.0, 1.0, 1.0)).expand(within, 10.0, within);
            for other in entity.world.load().get_entities_at_box(&bounds) {
                if other.get_entity().entity_id == entity.entity_id
                    || other.get_entity().entity_type != &EntityType::ZOMBIFIED_PIGLIN
                    || other.is_spectator()
                    || other.is_allied_to(target.as_ref())
                {
                    continue;
                }
                if let Some(mob) = other.get_mob()
                    && mob.get_mob_entity().get_target().is_none()
                {
                    mob.set_mob_target(Some(target.clone()));
                }
            }
        }
    }
}
