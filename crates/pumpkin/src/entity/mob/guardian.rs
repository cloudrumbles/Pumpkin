use std::sync::{Arc, Weak, atomic::Ordering};

use pumpkin_data::{damage::DamageType, entity::EntityType, sound::Sound, tracked_data};

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        active_target::ActiveTargetGoal,
        aquatic::{self, RandomSwimGoal},
        guardian_attack::{GuardianAttackGoal, is_guardian_prey},
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal,
    },
    custom_sound::CustomSound,
    mob::{Mob, MobEntity},
};

pub struct GuardianEntity {
    pub mob_entity: MobEntity,
}

impl GuardianEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let guardian = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
        });
        let mob: Arc<dyn Mob> = guardian.clone();
        register_guardian_ai(&guardian.mob_entity, Arc::downgrade(&mob), false);
        guardian
    }
}

pub(super) fn register_guardian_ai(mob: &MobEntity, owner: Weak<dyn Mob>, elder: bool) {
    aquatic::configure_swimming(mob);
    let mut goals = mob
        .goals_selector
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    goals.add_goal(4, Box::new(GuardianAttackGoal::new(elder)));
    goals.add_goal(
        7,
        Box::new(RandomSwimGoal::new(1.0, if elder { 400 } else { 80 })),
    );
    goals.add_goal(
        8,
        LookAtEntityGoal::with_default(owner.clone(), &EntityType::PLAYER, 8.0),
    );
    goals.add_goal(9, Box::new(RandomLookAroundGoal::default()));
    drop(goals);
    mob.target_selector
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .add_goal(
            1,
            ActiveTargetGoal::predicated(mob, 10, true, move |target, _world| {
                is_guardian_prey(target.entity.entity_type)
                    && owner.upgrade().is_some_and(|guardian| {
                        guardian
                            .get_entity()
                            .pos
                            .load()
                            .squared_distance_to_vec(&target.entity.pos.load())
                            > 9.0
                    })
            }),
        );
}

pub(super) fn update_guardian_movement(mob: &dyn Mob) {
    aquatic::flop_with_strength(mob, 0.5);
    let moving = mob
        .get_mob_entity()
        .living_entity
        .movement_input
        .load()
        .length_squared()
        > 0.0001;
    mob.get_entity()
        .set_synced_data(tracked_data::guardian::DATA_ID_MOVING, moving);
}

pub(super) fn guardian_thorns(
    mob: &dyn Mob,
    damage_type: DamageType,
    source: Option<&dyn EntityBase>,
) {
    if damage_type == DamageType::THORNS
        || damage_type == DamageType::MAGIC
        || damage_type == DamageType::INDIRECT_MAGIC
        || mob
            .get_mob_entity()
            .living_entity
            .movement_input
            .load()
            .length_squared()
            > 0.0001
    {
        return;
    }
    if let Some(source) = source {
        source.damage_with_context(
            source,
            2.0,
            DamageType::THORNS,
            None,
            Some(mob.get_entity()),
            Some(mob),
        );
    }
}

impl CustomSound for GuardianEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(
            if self.get_entity().touching_water.load(Ordering::Relaxed) {
                Sound::EntityGuardianHurt
            } else {
                Sound::EntityGuardianHurtLand
            },
        )
    }
    fn death_sound(&self) -> Option<Sound> {
        Some(
            if self.get_entity().touching_water.load(Ordering::Relaxed) {
                Sound::EntityGuardianDeath
            } else {
                Sound::EntityGuardianDeathLand
            },
        )
    }
}

impl Mob for GuardianEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }
    fn get_mob_gravity(&self) -> f64 {
        aquatic::aquatic_gravity(&self.mob_entity)
    }
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        update_guardian_movement(self);
    }
    fn on_damage(&self, damage_type: DamageType, source: Option<&dyn EntityBase>) {
        guardian_thorns(self, damage_type, source);
    }
}
