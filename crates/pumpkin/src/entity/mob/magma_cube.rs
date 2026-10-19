use std::sync::Arc;
use std::sync::atomic::Ordering;

use pumpkin_data::{attributes::Attributes, sound::Sound};
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::{
    Entity, EntityBase,
    ageable::AgeableMob,
    custom_sound::CustomSound,
    mob::{Mob, MobEntity, slime::SlimeEntity},
};

pub struct MagmaCubeEntity {
    pub slime: Arc<SlimeEntity>,
}

impl MagmaCubeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let slime = SlimeEntity::new(entity);
        let magma = Arc::new(Self { slime });
        magma.update_magma_attributes();
        magma
    }
}

impl MagmaCubeEntity {
    fn update_magma_attributes(&self) {
        let size = self.slime.get_size();
        let mut attributes = self
            .slime
            .get_mob_entity()
            .living_entity
            .attributes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (kind, value) in [
            (&Attributes::MOVEMENT_SPEED, 0.2),
            (&Attributes::ATTACK_DAMAGE, (size + 2) as f64),
            (&Attributes::ARMOR, (size * 3) as f64),
        ] {
            if let Some(attribute) = attributes.get_mut(&kind.id) {
                attribute.base_value = value;
                attribute.dirty.store(true, Ordering::Relaxed);
            }
        }
    }

    pub fn set_size(&self, size: i32, update_health: bool) {
        self.slime.set_size(size, update_health);
        self.update_magma_attributes();
    }
}

impl CustomSound for MagmaCubeEntity {
    fn death_sound(&self) -> Option<Sound> {
        let size = self.slime.get_size();
        Some(if size == 1 {
            Sound::EntityMagmaCubeDeathSmall
        } else {
            Sound::EntityMagmaCubeDeath
        })
    }
    fn hurt_sound(&self) -> Option<Sound> {
        let size = self.slime.get_size();
        Some(if size == 1 {
            Sound::EntityMagmaCubeHurtSmall
        } else {
            Sound::EntityMagmaCubeHurt
        })
    }
}

impl Mob for MagmaCubeEntity {
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self.slime.as_ref())
    }

    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }

    fn create_breed_offspring(
        &self,
        _world: &Arc<crate::world::World>,
    ) -> Option<Arc<dyn EntityBase>> {
        None
    }

    fn mob_init_data_tracker(&self) {
        self.slime.mob_init_data_tracker();
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.slime.mob_write_nbt(nbt);
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.slime.mob_read_nbt(nbt);
        self.update_magma_attributes();
    }

    fn get_mob_entity(&self) -> &MobEntity {
        self.slime.get_mob_entity()
    }

    fn mob_tick(&self, caller: &dyn EntityBase) {
        self.slime.mob_tick(caller);
    }

    fn post_tick(&self) {
        self.slime.post_tick();
    }

    fn mob_player_collision(&self, player: &Arc<crate::entity::player::Player>) {
        self.slime
            .get_mob_entity()
            .try_attack(&*self.slime, &**player);
    }
}
