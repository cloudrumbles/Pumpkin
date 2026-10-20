use std::sync::Arc;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::aquatic::{self, RandomSwimGoal},
    mob::{Mob, MobEntity},
};

pub struct CodEntity {
    pub mob_entity: MobEntity,
}

impl CodEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let school_size = aquatic::school_size(entity.entity_type);
        let mob_entity = MobEntity::new(entity);
        aquatic::configure_swimming(&mob_entity);
        mob_entity
            .goals_selector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .add_goal(4, Box::new(RandomSwimGoal::fish(school_size)));
        Arc::new(Self { mob_entity })
    }
}

impl Mob for CodEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
    fn get_mob_gravity(&self) -> f64 {
        aquatic::aquatic_gravity(&self.mob_entity)
    }
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        aquatic::flop(self);
    }
}
