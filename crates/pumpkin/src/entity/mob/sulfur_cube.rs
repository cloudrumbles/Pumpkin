//! Sulfur cubes hop and seek swallowable blocks until they acquire body equipment.
//! Their inert block forms use the 26.3 datapack's archetype attributes.
use crate::{
    entity::{
        Entity, EntityBase,
        ageable::{AgeableData, AgeableMob},
        attributes::{Modifier, ModifierOperation},
        custom_sound::CustomSound,
        item::ItemEntity,
        mob::{Mob, MobEntity},
        player::Player,
    },
    world::ExplosionInteraction,
};
use pumpkin_data::{
    attributes::Attributes,
    damage::DamageType,
    data_component_impl::EquipmentSlot,
    entity::EntityType,
    item::Item,
    item_stack::ItemStack,
    particle::Particle,
    sound::Sound,
    tag::{self, Taggable},
    tracked_data,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_util::math::{
    boundingbox::{BoundingBox, EntityDimensions},
    vector3::Vector3,
};
use serde::Deserialize;
use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicBool, AtomicI32, AtomicU16, Ordering},
};

#[derive(Deserialize)]
struct Archetype {
    items: String,
    #[serde(default)]
    attribute_modifiers: Vec<ArchetypeModifier>,
    #[serde(default)]
    buoyant: bool,
    explosion: Option<CubeExplosion>,
    contact_damage: Option<ContactDamage>,
    sound_settings: Option<CubeSounds>,
    knockback_modifiers: Option<CubeKnockback>,
}
#[derive(Deserialize)]
struct ArchetypeModifier {
    attribute: String,
    id: String,
    amount: f64,
    operation: String,
}
#[derive(Deserialize)]
struct CubeExplosion {
    fuse: i32,
    power: f32,
}
#[derive(Deserialize)]
struct ContactDamage {
    amount: f32,
    damage_type: String,
    attribute_to_source: bool,
}
#[derive(Deserialize)]
struct CubeSounds {
    hit_sound: String,
    push_sound: String,
    push_sound_cooldown: f64,
    push_sound_impulse_threshold: f64,
}
#[derive(Deserialize)]
struct CubeKnockback {
    horizontal_power: f64,
    vertical_power: f64,
}

#[expect(
    clippy::expect_used,
    reason = "These versioned datapack assets are validated at build/test time."
)]
static ARCHETYPES: LazyLock<Vec<Archetype>> = LazyLock::new(|| {
    [
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/datapack/data/minecraft/sulfur_cube_archetype/bouncy.json"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/datapack/data/minecraft/sulfur_cube_archetype/explosive.json"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/datapack/data/minecraft/sulfur_cube_archetype/fast_flat.json"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/datapack/data/minecraft/sulfur_cube_archetype/fast_sliding.json"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/datapack/data/minecraft/sulfur_cube_archetype/high_resistance.json"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/datapack/data/minecraft/sulfur_cube_archetype/hot.json"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/datapack/data/minecraft/sulfur_cube_archetype/light.json"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/datapack/data/minecraft/sulfur_cube_archetype/regular.json"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/datapack/data/minecraft/sulfur_cube_archetype/slow_bouncy.json"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/datapack/data/minecraft/sulfur_cube_archetype/slow_flat.json"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/datapack/data/minecraft/sulfur_cube_archetype/slow_sliding.json"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/datapack/data/minecraft/sulfur_cube_archetype/sticky.json"
        )),
    ]
    .into_iter()
    .map(|json| serde_json::from_str(json).expect("invalid shipped sulfur cube archetype"))
    .collect()
});

struct Hopping {
    yaw: f32,
    next_turn: i64,
    jump_delay: i32,
    previous_velocity: Vector3<f64>,
    was_on_ground: bool,
    push_cooldown: i32,
}

pub struct SulfurCubeEntity {
    pub mob_entity: MobEntity,
    ageable_data: AgeableData,
    size: AtomicI32,
    pickup_timer: AtomicI32,
    from_bucket: AtomicBool,
    fuse: AtomicI32,
    has_split: AtomicBool,
    body_item: AtomicU16,
    archetypes: Mutex<Vec<usize>>,
    hopping: Mutex<Hopping>,
}

impl SulfurCubeEntity {
    pub fn new(entity: Entity) -> Arc<Self> {
        let cube = Arc::new(Self {
            mob_entity: MobEntity::new(entity),
            ageable_data: AgeableData::default(),
            size: AtomicI32::new(2),
            pickup_timer: AtomicI32::new(0),
            from_bucket: AtomicBool::new(false),
            fuse: AtomicI32::new(-1),
            has_split: AtomicBool::new(false),
            body_item: AtomicU16::new(Item::AIR.id),
            archetypes: Mutex::new(Vec::new()),
            hopping: Mutex::new(Hopping {
                yaw: 0.0,
                next_turn: 0,
                jump_delay: 0,
                previous_velocity: Vector3::new(0.0, 0.0, 0.0),
                was_on_ground: false,
                push_cooldown: 0,
            }),
        });
        cube.set_size(2, true);
        cube
    }

    fn set_size(&self, size: i32, heal: bool) {
        let size = size.clamp(1, 2);
        self.size.store(size, Ordering::Relaxed);
        let living = &self.mob_entity.living_entity;
        living.set_attribute_base(&Attributes::MAX_HEALTH, f64::from(4 * size));
        living.set_attribute_base(&Attributes::MOVEMENT_SPEED, 0.2 + 0.1 * f64::from(size));
        if heal {
            living.set_health(living.get_max_health());
        }
        let entity = self.get_entity();
        entity.set_synced_data(tracked_data::sulfur_cube::ID_SIZE, VarInt(size));
        let dimensions = EntityDimensions {
            width: entity.entity_type.dimension[0] * size as f32,
            height: entity.entity_type.dimension[1] * size as f32,
            eye_height: entity.entity_type.eye_height * size as f32,
        };
        entity.entity_dimension.store(dimensions);
        let pos = entity.pos.load();
        entity
            .bounding_box
            .store(BoundingBox::new_from_pos(pos.x, pos.y, pos.z, &dimensions));
    }

    fn body_stack(&self) -> ItemStack {
        self.mob_entity
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&EquipmentSlot::BODY)
    }
    fn has_body_item(&self) -> bool {
        !self.body_stack().is_empty()
    }
    fn matching_archetypes(&self) -> Vec<usize> {
        self.archetypes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn refresh_contents(&self) {
        let stack = self.body_stack();
        let item = if stack.is_empty() {
            Item::AIR.id
        } else {
            stack.item.id
        };
        if self.body_item.swap(item, Ordering::Relaxed) == item {
            return;
        }
        let living = &self.mob_entity.living_entity;
        let previous = self.matching_archetypes();
        for index in previous {
            for modifier in &ARCHETYPES[index].attribute_modifiers {
                if let Some(attribute) = Attributes::ALL
                    .iter()
                    .find(|attribute| attribute.name == modifier.attribute)
                {
                    living.update_attribute(attribute, |instance| {
                        instance.remove_modifier(&modifier.id)
                    });
                }
            }
        }
        let matching: Vec<usize> = ARCHETYPES
            .iter()
            .enumerate()
            .filter_map(|(index, archetype)| {
                (!stack.is_empty() && stack.item.is_tagged_with(&archetype.items).unwrap_or(false))
                    .then_some(index)
            })
            .collect();
        for &index in &matching {
            for modifier in &ARCHETYPES[index].attribute_modifiers {
                if let Some(attribute) = Attributes::ALL
                    .iter()
                    .find(|attribute| attribute.name == modifier.attribute)
                {
                    let operation = match modifier.operation.as_str() {
                        "add_multiplied_base" => ModifierOperation::MultiplyBase,
                        "add_multiplied_total" => ModifierOperation::MultiplyTotal,
                        _ => ModifierOperation::Add,
                    };
                    living.update_attribute(attribute, |instance| {
                        instance.add_or_replace_modifier(Modifier {
                            id: modifier.id.clone(),
                            amount: modifier.amount,
                            operation,
                        })
                    });
                }
            }
        }
        *self
            .archetypes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = matching;
        living.set_attribute_base(
            &Attributes::STEP_HEIGHT,
            if stack.is_empty() { 0.6 } else { 0.0 },
        );
        living.movement_input.store(Vector3::new(0.0, 0.0, 0.0));
        living.jumping.store(false, Ordering::Relaxed);
    }

    fn drop_stack(&self, stack: ItemStack) {
        if stack.is_empty() {
            return;
        }
        let entity = self.get_entity();
        let world = entity.world.load();
        let item = ItemEntity::new(
            Entity::new(
                world.clone(),
                entity.pos.load() + Vector3::new(0.0, 0.5, 0.0),
                &EntityType::ITEM,
            ),
            stack,
        );
        world.spawn_entity_non_save(Arc::new(item));
    }

    fn set_body_stack(&self, stack: ItemStack) -> bool {
        let living = &self.mob_entity.living_entity;
        let previous = {
            let mut equipment = living
                .entity_equipment
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = equipment.get(&EquipmentSlot::BODY);
            if !previous.is_empty() && previous.item.id == stack.item.id {
                return false;
            }
            equipment.put(&EquipmentSlot::BODY, stack.clone());
            previous
        };
        self.drop_stack(previous);
        living
            .equipment_drop_chances
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(EquipmentSlot::BODY, 2.0);
        living.send_equipment_changes(&[(EquipmentSlot::BODY, stack)]);
        self.refresh_contents();
        true
    }

    fn prime(&self, imminent: bool) -> bool {
        let entity = self.get_entity();
        if !entity.is_alive()
            || self.fuse.load(Ordering::Relaxed) >= 0
            || !entity
                .world
                .load()
                .level_info
                .load()
                .game_rules
                .tnt_explodes
        {
            return false;
        }
        let indices = self.matching_archetypes();
        let Some(explosion) = indices
            .iter()
            .rev()
            .find_map(|&index| ARCHETYPES[index].explosion.as_ref())
        else {
            return false;
        };
        let fuse = if imminent {
            rand::random_range(explosion.fuse / 8..explosion.fuse / 4)
        } else {
            explosion.fuse
        };
        if self
            .fuse
            .compare_exchange(-1, fuse, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return false;
        }
        entity.set_synced_data(tracked_data::sulfur_cube::MAX_FUSE, VarInt(fuse));
        entity.play_sound(Sound::EntityTntPrimed);
        true
    }

    fn seek_target(&self) -> Option<Vector3<f64>> {
        let entity = self.get_entity();
        let world = entity.world.load();
        let accepts = |stack: &ItemStack| {
            stack.item.has_tag(if self.is_baby() {
                &tag::Item::MINECRAFT_SULFUR_CUBE_FOOD
            } else {
                &tag::Item::MINECRAFT_SULFUR_CUBE_SWALLOWABLE
            })
        };
        if let Some(player) = world.get_nearest_player(entity.pos.load(), 8.0, |player| {
            !player.is_spectator()
                && (accepts(&player.inventory().held_item())
                    || accepts(&player.inventory().off_hand_item()))
        }) {
            return Some(player.get_entity().pos.load());
        }
        if self.is_baby()
            || self.pickup_timer.load(Ordering::Relaxed) > 0
            || !world.level_info.load().game_rules.mob_griefing
        {
            return None;
        }
        let target = world.get_nearest_entity(
            entity.pos.load(),
            8.0,
            Some(&[&EntityType::ITEM]),
            |candidate| {
                candidate.get_item_entity().is_some_and(|item| {
                    item.get_pickup_delay() == 0
                        && accepts(
                            &item
                                .get_item_stack()
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner),
                        )
                })
            },
        )?;
        if entity
            .bounding_box
            .load()
            .expand(0.1, 0.1, 0.1)
            .intersects(&target.get_entity().bounding_box.load())
            && let Some(item) = target.get_item_entity()
            && let Some(stack) = item.take_for_mob(&self.mob_entity.living_entity, 1, accepts)
        {
            self.set_body_stack(stack);
            entity.play_sound(Sound::EntitySulfurCubeAbsorb);
        }
        Some(target.get_entity().pos.load())
    }
}

impl AgeableMob for SulfurCubeEntity {
    fn get_ageable_data(&self) -> &AgeableData {
        &self.ageable_data
    }
}
impl CustomSound for SulfurCubeEntity {
    fn hurt_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntitySmallSulfurCubeHurt
        } else {
            Sound::EntitySulfurCubeHurt
        })
    }
    fn death_sound(&self) -> Option<Sound> {
        Some(if self.is_baby() {
            Sound::EntitySmallSulfurCubeDeath
        } else {
            Sound::EntitySulfurCubeDeath
        })
    }
}
impl Mob for SulfurCubeEntity {
    fn get_mob_entity(&self) -> &MobEntity {
        &self.mob_entity
    }
    fn as_ageable(&self) -> Option<&dyn AgeableMob> {
        Some(self)
    }
    fn as_custom_sound(&self) -> Option<&dyn CustomSound> {
        Some(self)
    }
    fn requires_custom_persistence(&self) -> bool {
        self.from_bucket.load(Ordering::Relaxed) || self.has_body_item()
    }
    fn get_mob_y_velocity_drag(&self) -> Option<f64> {
        self.has_body_item().then_some(0.91)
    }
    fn mob_init_data_tracker(&self) {
        self.get_entity().set_synced_data(
            tracked_data::sulfur_cube::ID_SIZE,
            VarInt(self.size.load(Ordering::Relaxed)),
        );
        self.get_entity().set_synced_data(
            tracked_data::sulfur_cube::FROM_BUCKET,
            self.from_bucket.load(Ordering::Relaxed),
        );
        self.get_entity().set_synced_data(
            tracked_data::sulfur_cube::MAX_FUSE,
            VarInt(self.fuse.load(Ordering::Relaxed)),
        );
    }
    fn mob_tick(&self, _caller: &dyn EntityBase) {
        self.ageable_ai_step();
        let size = if self.is_baby() { 1 } else { 2 };
        if self.size.load(Ordering::Relaxed) != size {
            self.set_size(size, true);
        }
        self.refresh_contents();
        let entity = self.get_entity();
        self.hopping
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .previous_velocity = entity.velocity.load();
        if self.fuse.load(Ordering::Relaxed) > 0 && self.fuse.fetch_sub(1, Ordering::Relaxed) == 1 {
            let world = entity.world.load();
            let power = self.matching_archetypes().iter().find_map(|&index| {
                ARCHETYPES[index]
                    .explosion
                    .as_ref()
                    .map(|explosion| explosion.power)
            });
            self.has_split.store(true, Ordering::Relaxed);
            self.mob_entity
                .living_entity
                .dead
                .store(true, Ordering::Relaxed);
            if world.level_info.load().game_rules.tnt_explodes
                && let Some(power) = power
            {
                let interaction = if world.level_info.load().game_rules.mob_griefing {
                    ExplosionInteraction::Tnt
                } else {
                    ExplosionInteraction::None
                };
                world.explode(entity.pos.load(), power, interaction);
            }
            entity.remove();
        } else if self.has_body_item()
            && crate::block::blocks::redstone::block_receives_redstone_power(
                &entity.world.load(),
                &entity.block_pos.load(),
            )
        {
            self.prime(false);
        }
    }
    fn custom_server_ai_step(&self, _caller: &dyn EntityBase) {
        if self.pickup_timer.load(Ordering::Relaxed) > 0 {
            self.pickup_timer.fetch_sub(1, Ordering::Relaxed);
        }
        let entity = self.get_entity();
        if self.has_body_item() {
            let water = entity.touching_water.load(Ordering::Relaxed)
                || entity.touching_lava.load(Ordering::Relaxed);
            if water
                && self
                    .matching_archetypes()
                    .iter()
                    .any(|&index| ARCHETYPES[index].buoyant)
            {
                entity
                    .velocity
                    .store(entity.velocity.load() + Vector3::new(0.0, 0.04, 0.0));
            }
            entity.yaw.store((entity.yaw.load() / 90.0).round() * 90.0);
            self.mob_entity
                .living_entity
                .movement_input
                .store(Vector3::new(0.0, 0.0, 0.0));
            return;
        }
        let target = self.seek_target();
        if self.has_body_item() {
            return;
        }
        let mut hop = self
            .hopping
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = entity.world.load().get_world_age();
        if let Some(target) = target {
            let delta = target - entity.pos.load();
            hop.yaw = -delta.x.atan2(delta.z).to_degrees() as f32;
        } else if now >= hop.next_turn {
            hop.yaw = rand::random_range(0.0..360.0);
            hop.next_turn = now + rand::random_range(40..100);
        }
        entity.yaw.store(hop.yaw);
        entity.head_yaw.store(hop.yaw);
        entity.body_yaw.store(hop.yaw);
        let living = &self.mob_entity.living_entity;
        if entity.touching_water.load(Ordering::Relaxed)
            || entity.touching_lava.load(Ordering::Relaxed)
        {
            entity
                .velocity
                .store(entity.velocity.load() + Vector3::new(0.0, 0.04, 0.0));
            living.set_ai_speed(living.get_attribute_value(&Attributes::MOVEMENT_SPEED) * 1.2);
        } else if entity.on_ground.load(Ordering::Relaxed) {
            hop.jump_delay -= 1;
            let jump = hop.jump_delay <= 0;
            living.jumping.store(jump, Ordering::Relaxed);
            living.set_ai_speed(if jump {
                living.get_attribute_value(&Attributes::MOVEMENT_SPEED)
            } else {
                0.0
            });
            if jump {
                hop.jump_delay = rand::random_range(10..30) / if target.is_some() { 3 } else { 1 };
                entity.play_sound(if self.is_baby() {
                    Sound::EntitySmallSulfurCubeJump
                } else {
                    Sound::EntitySulfurCubeJump
                });
            }
        } else {
            living.jumping.store(false, Ordering::Relaxed);
            living.set_ai_speed(living.get_attribute_value(&Attributes::MOVEMENT_SPEED));
        }
    }
    fn post_tick(&self) {
        let entity = self.get_entity();
        let on_ground = entity.on_ground.load(Ordering::Relaxed);
        {
            let mut hop = self
                .hopping
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if on_ground && !hop.was_on_ground {
                entity.play_sound(if self.has_body_item() {
                    Sound::EntitySulfurCubeBounce
                } else if self.is_baby() {
                    Sound::EntitySmallSulfurCubeSquish
                } else {
                    Sound::EntitySulfurCubeSquish
                });
                if !self.has_body_item() {
                    entity.world.load().spawn_particle(
                        entity.pos.load(),
                        Vector3::new(0.5, 0.0, 0.5),
                        0.0,
                        16,
                        Particle::SulfurCubeGoo,
                    );
                }
            }
            let bounce = self
                .mob_entity
                .living_entity
                .get_attribute_value(&Attributes::BOUNCINESS);
            if bounce > 0.0 && self.has_body_item() && on_ground && hop.previous_velocity.y < -0.05
            {
                let mut velocity = entity.velocity.load();
                velocity.y = -hop.previous_velocity.y * bounce;
                entity.velocity.store(velocity);
                entity.on_ground.store(false, Ordering::Relaxed);
            }
            hop.was_on_ground = on_ground;
            hop.push_cooldown = (hop.push_cooldown - 1).max(0);
        }
        if self.has_body_item() {
            for target in entity
                .world
                .load()
                .get_all_at_box(&entity.bounding_box.load().expand(0.01, 0.01, 0.01))
            {
                if target.get_entity().entity_id == entity.entity_id
                    || target.get_living_entity().is_none()
                {
                    continue;
                }
                for index in self.matching_archetypes() {
                    if let Some(damage) = &ARCHETYPES[index].contact_damage
                        && let Some(kind) = DamageType::from_name(&damage.damage_type)
                    {
                        let source: Option<&dyn EntityBase> = (damage.attribute_to_source
                            || target.get_player().is_some())
                        .then_some(self);
                        target.damage_with_context(
                            target.as_ref(),
                            damage.amount,
                            kind,
                            Some(entity.pos.load()),
                            source,
                            source,
                        );
                    }
                }
            }
        }
        if self.mob_entity.living_entity.dead.load(Ordering::Relaxed)
            && !self.is_baby()
            && !self.has_split.swap(true, Ordering::Relaxed)
            && self.fuse.load(Ordering::Relaxed) < 0
        {
            let world = entity.world.load();
            for offset in [-0.25, 0.25] {
                let baby = Self::new(Entity::new(
                    world.clone(),
                    entity.pos.load() + Vector3::new(offset, 0.5, 0.0),
                    &EntityType::SULFUR_CUBE,
                ));
                baby.set_baby(true);
                baby.set_size(1, true);
                world.spawn_entity_non_save(baby);
            }
        }
    }
    fn pre_damage(&self, damage_type: DamageType, source: Option<&dyn EntityBase>) -> bool {
        if self.has_body_item() {
            if damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_FIRE) {
                self.prime(false);
            }
            if damage_type.has_tag(&tag::DamageType::MINECRAFT_IS_EXPLOSION) {
                self.prime(true);
            }
            if damage_type.has_tag(&tag::DamageType::MINECRAFT_SULFUR_CUBE_WITH_BLOCK_IMMUNE_TO) {
                if let Some(source) = source {
                    let delta = self.get_entity().pos.load() - source.get_entity().pos.load();
                    for index in self.matching_archetypes() {
                        if let Some(knockback) = &ARCHETYPES[index].knockback_modifiers {
                            let direction = Vector3::new(delta.x, 0.0, delta.z).normalize();
                            self.get_entity().velocity.store(
                                self.get_entity().velocity.load()
                                    + direction * knockback.horizontal_power
                                    + Vector3::new(0.0, knockback.vertical_power, 0.0),
                            );
                        }
                        if let Some(sounds) = &ARCHETYPES[index].sound_settings
                            && let Some(sound) =
                                Sound::from_name(sounds.hit_sound.trim_start_matches("minecraft:"))
                        {
                            self.get_entity().play_sound(sound);
                        }
                    }
                }
                return false;
            }
        }
        self.fuse.load(Ordering::Relaxed) < 0 || damage_type == DamageType::GENERIC_KILL
    }
    fn mob_player_collision(&self, player: &Arc<Player>) {
        if !self.has_body_item() || player.is_spectator() {
            return;
        }
        let entity = self.get_entity();
        let delta = entity.pos.load() - player.get_entity().pos.load();
        let direction = Vector3::new(delta.x, 0.0, delta.z).normalize();
        let resistance = (1.0
            - self
                .mob_entity
                .living_entity
                .get_attribute_value(&Attributes::KNOCKBACK_RESISTANCE))
        .max(0.0);
        let speed = (player.get_entity().velocity.load().length() * 0.6).min(0.5);
        let impulse = (direction
            + Vector3::new(
                0.0,
                if entity.on_ground.load(Ordering::Relaxed) {
                    0.3
                } else {
                    0.0
                },
                0.0,
            ))
            * (speed * resistance);
        entity.velocity.store(entity.velocity.load() + impulse);
        let mut hop = self
            .hopping
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for index in self.matching_archetypes() {
            if let Some(sounds) = &ARCHETYPES[index].sound_settings
                && hop.push_cooldown == 0
                && impulse.length() > sounds.push_sound_impulse_threshold
            {
                hop.push_cooldown = (sounds.push_sound_cooldown * 20.0) as i32;
                if let Some(sound) =
                    Sound::from_name(sounds.push_sound.trim_start_matches("minecraft:"))
                {
                    entity.play_sound(sound);
                }
            }
        }
    }
    fn mob_interact(&self, player: &Arc<Player>, stack: &mut ItemStack) -> bool {
        if self.fuse.load(Ordering::Relaxed) >= 0 {
            return false;
        }
        if self.is_baby() {
            if stack.item.has_tag(&tag::Item::MINECRAFT_SULFUR_CUBE_FOOD) && self.can_age_up() {
                self.age_up(
                    Self::get_speed_up_seconds_when_feeding(-self.get_age()),
                    true,
                );
                stack.decrement_unless_creative(player.gamemode.load(), 1);
                return true;
            }
            return false;
        }
        if (stack.item.id == Item::FLINT_AND_STEEL.id || stack.item.id == Item::FIRE_CHARGE.id)
            && self.prime(false)
        {
            if stack.item.id == Item::FIRE_CHARGE.id {
                stack.decrement_unless_creative(player.gamemode.load(), 1);
            }
            return true;
        }
        if stack.item.id == Item::SHEARS.id && self.has_body_item() {
            self.set_body_stack(ItemStack::EMPTY.clone());
            self.pickup_timer.store(100, Ordering::Relaxed);
            self.get_entity().play_sound(Sound::EntitySulfurCubeEject);
            return true;
        }
        if stack
            .item
            .has_tag(&tag::Item::MINECRAFT_SULFUR_CUBE_SWALLOWABLE)
        {
            let mut swallowed = stack.clone();
            swallowed.item_count = 1;
            if self.set_body_stack(swallowed) {
                stack.decrement_unless_creative(player.gamemode.load(), 1);
                self.get_entity().play_sound(Sound::EntitySulfurCubeAbsorb);
                return true;
            }
        }
        false
    }
    fn mob_write_nbt(&self, nbt: &mut NbtCompound) {
        self.write_ageable_nbt(nbt);
        nbt.put_int("Size", self.size.load(Ordering::Relaxed) - 1);
        nbt.put_int("pickup_timer", self.pickup_timer.load(Ordering::Relaxed));
        nbt.put_bool("from_bucket", self.from_bucket.load(Ordering::Relaxed));
        nbt.put_int("fuse", self.fuse.load(Ordering::Relaxed));
    }
    fn mob_read_nbt(&self, nbt: &NbtCompound) {
        self.read_ageable_nbt(nbt);
        if nbt.get_int("Size") == Some(0) {
            self.set_baby(true);
        }
        self.set_size(if self.is_baby() { 1 } else { 2 }, false);
        self.pickup_timer.store(
            nbt.get_int("pickup_timer").unwrap_or(0).max(0),
            Ordering::Relaxed,
        );
        self.from_bucket.store(
            nbt.get_bool("from_bucket").unwrap_or(false),
            Ordering::Relaxed,
        );
        self.fuse
            .store(nbt.get_int("fuse").unwrap_or(-1), Ordering::Relaxed);
        self.refresh_contents();
    }
}
