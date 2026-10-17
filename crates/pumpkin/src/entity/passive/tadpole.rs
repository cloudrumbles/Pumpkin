//! Tadpoles have an independent 0..24000 growth clock, unlike ageable mammals.
use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        aquatic::{self, RandomSwimGoal},
        escape_danger::EscapeDangerGoal,
        tempt::TemptGoal,
    },
    mob::{Mob, MobEntity},
    player::Player,
};
use pumpkin_data::{
    entity::EntityType,
    item::Item,
    item_stack::ItemStack,
    particle::Particle,
    sound::Sound,
    tag::{self, Taggable},
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::math::vector3::Vector3;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, Ordering},
};

pub struct TadpoleEntity {
    pub mob_entity: MobEntity,
    growth_age: AtomicI32,
    age_locked: AtomicBool,
}
impl TadpoleEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        aquatic::configure_swimming(&mob_entity);
        {
            let mut goals = mob_entity
                .goals_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            goals.add_goal(1, EscapeDangerGoal::new(2.0));
            goals.add_goal(
                2,
                Box::new(TemptGoal::with_tag(
                    1.25,
                    &tag::Item::MINECRAFT_FROG_FOOD,
                    2.5,
                )),
            );
            goals.add_goal(3, Box::new(RandomSwimGoal::new(0.5, 40)));
        }
        Arc::new(Self {
            mob_entity,
            growth_age: AtomicI32::new(0),
            age_locked: AtomicBool::new(false),
        })
    }
    fn grow_up(&self) {
        let entity = self.get_entity();
        if !self.mob_entity.living_entity.is_alive() {
            return;
        }
        let world = entity.world.load_full();
        let frog = crate::entity::r#type::from_type(
            &EntityType::FROG,
            entity.pos.load(),
            &world,
            uuid::Uuid::new_v4(),
        );
        let biome = world.get_biome(&entity.block_pos.load());
        let variant = if biome.has_tag(&tag::WorldgenBiome::MINECRAFT_SPAWNS_COLD_VARIANT_FROGS) {
            "minecraft:cold"
        } else if biome.has_tag(&tag::WorldgenBiome::MINECRAFT_SPAWNS_WARM_VARIANT_FROGS) {
            "minecraft:warm"
        } else {
            "minecraft:temperate"
        };
        frog.set_variant_name(variant);
        frog.get_entity()
            .set_rotation(entity.yaw.load(), entity.pitch.load());
        if let Some(name) = &**entity.custom_name.load() {
            frog.get_entity().set_custom_name(name.clone());
        }
        if let Some(mob) = frog.get_mob() {
            mob.get_mob_entity()
                .persistence_required
                .store(true, Ordering::Relaxed);
        }
        world.spawn_entity(frog);
        entity.play_sound(Sound::EntityTadpoleGrowUp);
        entity.remove();
    }
}
impl Mob for TadpoleEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
    fn requires_custom_persistence(&self) -> bool {
        true
    }
    fn get_mob_gravity(&self) -> f64 {
        aquatic::aquatic_gravity(&self.mob_entity)
    }
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        aquatic::flop(self);
    }
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        if !self.age_locked.load(Ordering::Relaxed)
            && self.growth_age.fetch_add(1, Ordering::Relaxed) + 1 >= 24000
        {
            self.grow_up();
        }
    }
    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_int("Age", self.growth_age.load(Ordering::Relaxed));
        nbt.put_bool("AgeLocked", self.age_locked.load(Ordering::Relaxed));
    }
    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.growth_age
            .store(nbt.get_int("Age").unwrap_or(0).max(0), Ordering::Relaxed);
        self.age_locked.store(
            nbt.get_bool("AgeLocked").unwrap_or(false),
            Ordering::Relaxed,
        );
    }
    fn mob_init_data_tracker(&self) {
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::tadpole::AGE_LOCKED,
            self.age_locked.load(Ordering::Relaxed),
        );
    }
    fn mob_interact(&self, player: &Arc<Player>, stack: &mut ItemStack) -> bool {
        if stack.item.has_tag(&tag::Item::MINECRAFT_FROG_FOOD)
            && !self.age_locked.load(Ordering::Relaxed)
        {
            stack.decrement_unless_creative(player.gamemode.load(), 1);
            let left = (24000 - self.growth_age.load(Ordering::Relaxed)).max(0);
            self.growth_age
                .fetch_add((left / 200) * 20, Ordering::Relaxed);
            let entity = self.get_entity();
            entity.world.load().spawn_particle(
                entity.pos.load() + Vector3::new(0.0, 0.5, 0.0),
                Vector3::new(0.2, 0.2, 0.2),
                0.0,
                1,
                Particle::HappyVillager,
            );
            if self.growth_age.load(Ordering::Relaxed) >= 24000 {
                self.grow_up();
            }
            return true;
        }
        if stack.item == &Item::GOLDEN_DANDELION {
            stack.decrement_unless_creative(player.gamemode.load(), 1);
            let locked = !self.age_locked.fetch_xor(true, Ordering::Relaxed);
            self.growth_age.store(0, Ordering::Relaxed);
            self.get_entity()
                .set_synced_data(pumpkin_data::tracked_data::tadpole::AGE_LOCKED, locked);
            return true;
        }
        crate::item::items::bucket::try_capture_mob(
            player,
            stack,
            self,
            &Item::TADPOLE_BUCKET,
            Sound::ItemBucketFillTadpole,
        )
    }
}
