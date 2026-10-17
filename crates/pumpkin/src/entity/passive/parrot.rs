use std::sync::{
    Arc, Weak,
    atomic::{AtomicI32, Ordering},
};

use pumpkin_data::damage::DamageType;
use pumpkin_data::effect::StatusEffect;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;

use crate::entity::{
    Entity, EntityBase,
    ai::goal::{
        escape_danger::EscapeDangerGoal, follow_mob::FollowMobGoal, follow_owner::FollowOwnerGoal,
        look_around::RandomLookAroundGoal, look_at_entity::LookAtEntityGoal,
        sit_when_ordered_to::SitWhenOrderedToGoal, swim::SwimGoal,
        water_avoiding_random_flying::WaterAvoidingRandomFlyingGoal,
    },
    ai::pathfinder::Navigator,
    mob::{Mob, MobEntity},
    passive::{
        animal::Animal,
        tamable::{TamableAnimal, TamableData},
    },
    player::Player,
};

/// Duration in ticks of the poison a parrot gets from eating a cookie, matching
/// vanilla `Parrot.mobInteract`.
const COOKIE_POISON_DURATION: i32 = 900;

/// Represents a Parrot, a passive flying mob that can mimic nearby mob sounds.
///
/// Wiki: <https://minecraft.wiki/w/Parrot>
pub struct ParrotEntity {
    pub mob_entity: MobEntity,
    tamable_data: TamableData,
    variant: AtomicI32,
}

impl ParrotEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let mob_entity = MobEntity::new(entity);
        *mob_entity
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Navigator::flying();
        let parrot = Self {
            mob_entity,
            tamable_data: TamableData::default(),
            variant: AtomicI32::new(rand::random_range(0..5)),
        };
        let mob_arc = Arc::new(parrot);
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
            goal_selector.add_goal(0, EscapeDangerGoal::new(1.25));
            goal_selector.add_goal(2, Box::new(SitWhenOrderedToGoal::new()));
            goal_selector.add_goal(2, FollowOwnerGoal::new(1.0, 5.0, 1.0));
            goal_selector.add_goal(2, Box::new(WaterAvoidingRandomFlyingGoal::new(1.0)));
            goal_selector.add_goal(3, Box::new(FollowMobGoal::new(1.0, 3.0, 7.0)));
            goal_selector.add_goal(
                1,
                LookAtEntityGoal::with_default(mob_weak, &EntityType::PLAYER, 8.0),
            );
            goal_selector.add_goal(4, Box::new(RandomLookAroundGoal::default()));
        };

        mob_arc
    }

    /// Feeds the parrot a cookie: it is poisoned and then killed, as in vanilla
    /// `Parrot.mobInteract`.
    fn eat_cookie(&self, player: &Arc<Player>, item_stack: &mut ItemStack) {
        item_stack.decrement_unless_creative(player.gamemode.load(), 1);

        self.mob_entity
            .living_entity
            .add_effect(pumpkin_data::potion::Effect {
                effect_type: &StatusEffect::POISON,
                duration: COOKIE_POISON_DURATION,
                amplifier: 0,
                ambient: false,
                show_particles: true,
                show_icon: true,
                blend: true,
            });

        // Vanilla guards this call with `player.isCreative() || !this.isInvulnerable()`,
        // but `hurt` re-checks invulnerability itself and `player_attack` doesn't bypass
        // it, so the guard only skips a call that would do nothing anyway.
        self.damage_with_context(
            self,
            f32::MAX,
            DamageType::PLAYER_ATTACK,
            None,
            Some(player.as_ref()),
            Some(player.as_ref()),
        );
    }
}

impl Animal for ParrotEntity {
    fn is_food(&self, _stack: &ItemStack) -> bool {
        false
    }
}
impl TamableAnimal for ParrotEntity {
    fn get_tamable_data(&self) -> &TamableData {
        &self.tamable_data
    }
}

impl Mob for ParrotEntity {
    fn as_tamable(&self) -> Option<&dyn TamableAnimal> {
        Some(self)
    }
    fn requires_custom_persistence(&self) -> bool {
        self.is_tame()
    }
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }

    fn mob_interact(&self, player: &Arc<Player>, item_stack: &mut ItemStack) -> bool {
        if !self.is_tame() && item_stack.item.has_tag(&tag::Item::MINECRAFT_PARROT_FOOD) {
            item_stack.decrement_unless_creative(player.gamemode.load(), 1);
            self.get_entity().play_sound(Sound::EntityParrotEat);
            let success = rand::random_range(0..10) == 0;
            if success {
                TamableAnimal::tame(self, player.gameprofile.id);
            }
            self.spawn_taming_particles(success);
            return true;
        }
        if item_stack
            .item
            .has_tag(&tag::Item::MINECRAFT_PARROT_POISONOUS_FOOD)
        {
            self.eat_cookie(player, item_stack);
            return true;
        }
        if self.is_tame()
            && self.is_owned_by(&player.gameprofile.id)
            && self.get_entity().on_ground.load(Ordering::Relaxed)
        {
            self.set_ordered_to_sit(!self.is_ordered_to_sit());
            return true;
        }
        self.mob_entity.mob_interact(player, item_stack)
    }

    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        let entity = self.get_entity();
        if self.is_navigator_idle() {
            entity.set_has_no_gravity(false);
        }
        if entity.is_silent() || rand::random_range(0..800) != 0 {
            return;
        }
        let world = entity.world.load();
        let sounds: Vec<Sound> = world
            .get_entities_at_box(&entity.bounding_box.load().expand(20.0, 20.0, 20.0))
            .iter()
            .filter(|mob| !mob.get_entity().is_silent() && mob.get_living_entity().is_some())
            .filter_map(|mob| {
                let kind = mob.get_entity().entity_type.resource_name;
                let kind = if kind == "cave_spider" {
                    "spider"
                } else {
                    kind
                };
                Sound::from_name(&format!("entity.parrot.imitate.{kind}"))
            })
            .collect();
        if !sounds.is_empty() {
            world.play_sound_fine(
                sounds[rand::random_range(0..sounds.len())],
                SoundCategory::Neutral,
                &entity.pos.load(),
                0.7,
                rand::random_range(0.8..1.2),
            );
        }
    }
    fn post_tick(&self) {
        let entity = self.get_entity();
        let velocity = entity.velocity.load();
        if !entity.on_ground.load(Ordering::Relaxed) && velocity.y < 0.0 {
            entity.velocity.store(velocity.multiply(1.0, 0.6, 1.0));
        }
        self.mob_entity.living_entity.fall_distance.store(0.0);
    }
    fn on_damage(&self, _damage_type: DamageType, _source: Option<&dyn EntityBase>) {
        self.set_ordered_to_sit(false);
    }
    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_tamable_nbt(nbt);
        nbt.put_int("Variant", self.variant.load(Ordering::Relaxed));
    }
    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_tamable_nbt(nbt);
        self.variant.store(
            nbt.get_int("Variant").unwrap_or(0).clamp(0, 4),
            Ordering::Relaxed,
        );
    }
    fn mob_init_data_tracker(&self) {
        self.get_entity().set_synced_data(
            pumpkin_data::tracked_data::parrot::DATA_VARIANT_ID,
            VarInt(self.variant.load(Ordering::Relaxed)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::COOKIE_POISON_DURATION;
    use pumpkin_data::item::Item;
    use pumpkin_data::tag::{self, Taggable};

    /// The interaction is gated on the vanilla `parrot_poisonous_food` tag rather than
    /// on a hardcoded cookie id, so check the tag actually resolves the way the
    /// interaction assumes.
    #[test]
    fn cookie_is_poisonous_parrot_food() {
        assert!(Item::COOKIE.has_tag(&tag::Item::MINECRAFT_PARROT_POISONOUS_FOOD));
    }

    /// Seeds tame a parrot in vanilla and must not reach the poison branch.
    #[test]
    fn parrot_food_is_not_poisonous() {
        assert!(!Item::WHEAT_SEEDS.has_tag(&tag::Item::MINECRAFT_PARROT_POISONOUS_FOOD));
        assert!(!Item::COOKED_CHICKEN.has_tag(&tag::Item::MINECRAFT_PARROT_POISONOUS_FOOD));
    }

    #[test]
    fn poison_lasts_45_seconds() {
        assert_eq!(COOKIE_POISON_DURATION, 900);
    }
}
