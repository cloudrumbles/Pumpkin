use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, AtomicU8, Ordering},
};

use crate::world::World;
use pumpkin_data::sound::Sound;
use pumpkin_data::{
    data_component_impl::EquipmentSlot, entity::EntityType, item::Item, item_stack::ItemStack,
};
use pumpkin_nbt::compound::NbtCompound;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        active_target::ActiveTargetGoal,
        avoid_entity::AvoidEntityGoal,
        bow_attack::BowAttackGoal,
        illusioner_spells::{IllusionerCast, IllusionerCastingGoal, IllusionerSpellGoal},
        look_around::RandomLookAroundGoal,
        look_at_entity::LookAtEntityGoal,
        revenge::RevengeGoal,
        swim::SwimGoal,
        wander_around::WanderAroundGoal,
    },
    mob::{
        Mob, MobEntity,
        equipment::RegionalDifficulty,
        evoker::IllagerSpell,
        patrol::{LongDistancePatrolGoal, PatrolData, PatrollingMonster},
        raider::{
            HoldGroundAttackGoal, ObtainRaidLeaderBannerGoal, PathfindToRaidGoal, Raider,
            RaiderCelebrationGoal, RaiderData, RaiderMoveThroughVillageGoal,
        },
    },
};

pub struct IllusionerEntity {
    pub mob_entity: MobEntity,
    pub raider_data: RaiderData,
    spell_casting_tick_count: AtomicI32,
    current_spell: AtomicU8,
}

impl IllusionerEntity {
    pub fn is_casting_spell(&self) -> bool {
        self.spell_casting_tick_count.load(Ordering::Relaxed) > 0
    }
    pub fn set_spell_casting_time(&self, ticks: i32) {
        self.spell_casting_tick_count
            .store(ticks.max(0), Ordering::Relaxed);
    }
    pub fn set_is_casting_spell(&self, spell: IllagerSpell) {
        self.current_spell.store(spell as u8, Ordering::Relaxed);
        self.mob_entity.living_entity.entity.set_synced_data(
            pumpkin_data::tracked_data::illusioner::SPELL_CASTING_ID,
            spell as u8 as i8,
        );
    }

    #[must_use]
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        let illusioner = Self {
            mob_entity,
            raider_data: RaiderData::default(),
            spell_casting_tick_count: AtomicI32::new(0),
            current_spell: AtomicU8::new(IllagerSpell::None as u8),
        };
        let mob_arc = Arc::new(illusioner);
        let spell_weak: Weak<IllusionerEntity> = Arc::downgrade(&mob_arc);
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
            goal_selector.add_goal(1, Box::new(IllusionerCastingGoal::new(spell_weak.clone())));
            goal_selector.add_goal(1, Box::new(ObtainRaidLeaderBannerGoal));
            goal_selector.add_goal(
                3,
                Box::new(AvoidEntityGoal::new(&EntityType::CREAKING, 8.0, 1.0, 1.2)),
            );
            goal_selector.add_goal(
                4,
                Box::new(IllusionerSpellGoal::new(
                    spell_weak.clone(),
                    IllusionerCast::Mirror,
                )),
            );
            goal_selector.add_goal(
                5,
                Box::new(IllusionerSpellGoal::new(
                    spell_weak,
                    IllusionerCast::Blindness,
                )),
            );
            goal_selector.add_goal(6, Box::new(BowAttackGoal::new(0.5, 20, 15.0)));
            goal_selector.add_goal(2, Box::new(HoldGroundAttackGoal::new(10.0)));
            goal_selector.add_goal(4, Box::new(LongDistancePatrolGoal::new(0.7, 0.595)));
            goal_selector.add_goal(4, Box::new(RaiderMoveThroughVillageGoal::new(1.05)));
            goal_selector.add_goal(4, Box::new(PathfindToRaidGoal::default()));
            goal_selector.add_goal(5, Box::new(RaiderCelebrationGoal));
            goal_selector.add_goal(8, Box::new(WanderAroundGoal::new(0.6)));
            goal_selector.add_goal(
                9,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 3.0),
            );
            goal_selector.add_goal(10, Box::new(RandomLookAroundGoal::default()));

            let mut target_selector = mob_arc
                .mob_entity
                .target_selector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            target_selector.add_goal(1, Box::new(RevengeGoal::new(false)));
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::PLAYER, true),
            );
            target_selector.add_goal(
                2,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::VILLAGER, true),
            );
            target_selector.add_goal(
                3,
                ActiveTargetGoal::with_default(&mob_arc.mob_entity, &EntityType::IRON_GOLEM, true),
            );
        };

        mob_arc
    }
}

impl Mob for IllusionerEntity {
    fn populate_default_equipment_slots(
        &self,
        _world: &Arc<World>,
        _difficulty: &RegionalDifficulty,
    ) {
        let bow = ItemStack::new(1, &Item::BOW);
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .put(&EquipmentSlot::MAIN_HAND, bow);
    }

    fn mob_tick(&self, _caller: &dyn EntityBase) {
        let ticks = self.spell_casting_tick_count.load(Ordering::Relaxed);
        if ticks > 0 {
            self.spell_casting_tick_count
                .store(ticks - 1, Ordering::Relaxed);
            if ticks == 1 {
                self.set_is_casting_spell(IllagerSpell::None);
            }
        }
    }

    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn as_patrolling_monster(&self) -> Option<&dyn PatrollingMonster> {
        Some(self)
    }

    fn as_raider(&self) -> Option<&dyn Raider> {
        Some(self)
    }

    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_raider_nbt(nbt);
        nbt.put_int(
            "SpellTicks",
            self.spell_casting_tick_count.load(Ordering::Relaxed),
        );
    }

    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_raider_nbt(nbt);
        if let Some(ticks) = nbt.get_int("SpellTicks") {
            self.set_spell_casting_time(ticks);
        }
    }
}

impl PatrollingMonster for IllusionerEntity {
    fn get_patrol_data(&self) -> &PatrolData {
        &self.raider_data.patrol_data
    }
}

impl Raider for IllusionerEntity {
    fn get_raider_data(&self) -> &RaiderData {
        &self.raider_data
    }

    fn get_celebrate_sound(&self) -> Sound {
        Sound::EntityEvokerCelebrate
    }
}
