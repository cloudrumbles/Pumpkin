use super::BlockEntity;
use crate::{
    entity::{EntityBase, passive::bee::BeeEntity},
    world::World,
};
use pumpkin_data::{
    Block, block_properties::BeeNestLikeProperties, entity::EntityType, sound::Sound,
};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::{chunk::io::Dirtiable, world::BlockFlags};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

pub struct BeehiveBlockEntity {
    pub position: BlockPos,
    pub bees: Mutex<Option<Vec<NbtTag>>>,
    pub flower_pos: Mutex<Option<BlockPos>>,
    dirty: AtomicBool,
}

impl BlockEntity for BeehiveBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let bees = nbt
            .get_list("bees")
            .or_else(|| nbt.get_list("Bees"))
            .map(<[_]>::to_vec);
        let flower_pos = nbt
            .get_int_array("flower_pos")
            .and_then(|arr| (arr.len() == 3).then(|| BlockPos::new(arr[0], arr[1], arr[2])))
            .or_else(|| {
                nbt.get_compound("flower_pos")
                    .or_else(|| nbt.get_compound("FlowerPos"))
                    .map(|c| {
                        BlockPos::new(
                            c.get_int("X").or_else(|| c.get_int("x")).unwrap_or(0),
                            c.get_int("Y").or_else(|| c.get_int("y")).unwrap_or(0),
                            c.get_int("Z").or_else(|| c.get_int("z")).unwrap_or(0),
                        )
                    })
            });
        Self {
            position,
            bees: Mutex::new(bees),
            flower_pos: Mutex::new(flower_pos),
            dirty: AtomicBool::new(false),
        }
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        if let Ok(b) = self.bees.lock()
            && let Some(b) = b.as_ref()
        {
            nbt.put_list("bees", b.clone());
        }
        if let Ok(fp) = self.flower_pos.lock()
            && let Some(fp) = fp.as_ref()
        {
            nbt.put(
                "flower_pos",
                pumpkin_nbt::tag::NbtTag::IntArray(vec![fp.0.x, fp.0.y, fp.0.z]),
            );
        }
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        if let Ok(bees) = self.bees.try_lock()
            && let Some(ref b) = *bees
        {
            nbt.put_list("bees", b.clone());
        }
        if let Ok(flower_pos) = self.flower_pos.try_lock()
            && let Some(ref fp) = *flower_pos
        {
            nbt.put(
                "flower_pos",
                pumpkin_nbt::tag::NbtTag::IntArray(vec![fp.0.x, fp.0.y, fp.0.z]),
            );
        }
        Some(nbt)
    }

    fn tick(&self, world: &Arc<World>) {
        self.release(world, false);
    }
    fn on_block_replaced(self: Arc<Self>, world: &Arc<World>, _position: &BlockPos) {
        self.release(world, true);
    }
    fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }
    fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::Relaxed);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl BeehiveBlockEntity {
    pub const ID: &'static str = "minecraft:beehive";
    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            bees: Mutex::new(None),
            flower_pos: Mutex::new(None),
            dirty: AtomicBool::new(false),
        }
    }
}

impl BeehiveBlockEntity {
    fn mark_dirty(&self, world: &Arc<World>) {
        self.dirty.store(true, Ordering::Relaxed);
        let _ = world
            .level
            .read_chunk_sync(&self.position.chunk_position(), |chunk| {
                chunk.mark_dirty(true)
            });
    }
    pub fn has_space(&self) -> bool {
        self.bees
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_none_or(|bees| bees.len() < 3)
    }
    pub fn entrance(&self, world: &World) -> Vector3<f64> {
        let (block, state) = world.get_block_and_state_id(&self.position);
        let offset = if block == &Block::BEEHIVE || block == &Block::BEE_NEST {
            BeeNestLikeProperties::from_state_id(state)
                .facing
                .to_offset()
        } else {
            Vector3::new(0, 0, 0)
        };
        self.position.to_f64()
            + Vector3::new(
                0.5 + f64::from(offset.x) * 0.9,
                0.2,
                0.5 + f64::from(offset.z) * 0.9,
            )
    }
    pub fn try_enter(&self, bee: &BeeEntity) -> bool {
        let entity = bee.get_entity();
        let world = entity.world.load();
        if !bee.mob_entity.living_entity.is_alive() || bee.mob_entity.is_no_ai() {
            return false;
        }
        let mut saved = NbtCompound::new();
        bee.write_nbt(&mut saved);
        for key in [
            "UUID",
            "Pos",
            "Motion",
            "Rotation",
            "Leash",
            "Passengers",
            "FallDistance",
            "OnGround",
            "Air",
            "HurtTime",
            "DeathTime",
        ] {
            saved.child_tags.remove(key);
        }
        saved.put_string("id", "minecraft:bee".to_string());
        let mut occupant = NbtCompound::new();
        occupant.put_compound("entity_data", saved);
        occupant.put_int("ticks_in_hive", 0);
        occupant.put_int(
            "min_ticks_in_hive",
            if bee.has_nectar() { 2400 } else { 600 },
        );
        {
            let mut stored = self
                .bees
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let bees = stored.get_or_insert_with(Vec::new);
            if bees.len() >= 3 {
                return false;
            }
            bees.push(NbtTag::Compound(occupant));
        }
        if let Some(pos) = bee.flower_position.load() {
            *self
                .flower_pos
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pos);
        }
        entity.play_sound(Sound::BlockBeehiveEnter);
        entity.remove();
        self.mark_dirty(&world);
        true
    }
    fn release(&self, world: &Arc<World>, emergency: bool) {
        let entrance = self.entrance(world);
        let (block, state) = world.get_block_and_state_id(&self.position);
        let is_hive = block == &Block::BEEHIVE || block == &Block::BEE_NEST;
        let front_open = world
            .get_block_state(&BlockPos::floored_v(entrance))
            .collision_shapes
            .is_empty();
        let can_release = emergency || (!world.bees_stay_in_hive(&self.position) && front_open);
        let mut ready = Vec::new();
        let mut any = false;
        {
            let mut stored = self
                .bees
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(bees) = stored.as_mut() {
                bees.retain_mut(|tag| {
                    let NbtTag::Compound(data) = tag else {
                        return true;
                    };
                    any = true;
                    let ticks = data
                        .get_int("ticks_in_hive")
                        .or_else(|| data.get_int("TicksInHive"))
                        .unwrap_or(0)
                        .saturating_add(1);
                    data.put_int("ticks_in_hive", ticks);
                    let minimum = data
                        .get_int("min_ticks_in_hive")
                        .or_else(|| data.get_int("MinOccupationTicks"))
                        .unwrap_or(600);
                    if can_release
                        && (emergency || ticks > minimum)
                        && let Some(bee) = data
                            .get_compound("entity_data")
                            .or_else(|| data.get_compound("EntityData"))
                    {
                        ready.push((bee.clone(), ticks));
                        return false;
                    }
                    true
                });
            }
        }
        for (mut saved, ticks) in ready {
            // Release gets a fresh UUID; position and motion always come from the hive.
            for key in ["UUID", "Pos", "Motion", "Rotation", "Passengers"] {
                saved.child_tags.remove(key);
            }
            let nectar = saved.get_bool("HasNectar").unwrap_or(false);
            if !saved.get_bool("AgeLocked").unwrap_or(false) {
                let age = saved.get_int("Age").unwrap_or(0);
                saved.put_int(
                    "Age",
                    if age < 0 {
                        (age + ticks).min(0)
                    } else {
                        (age - ticks).max(0)
                    },
                );
            }
            if nectar && !emergency {
                saved.put_bool("HasNectar", false);
                saved.put_int("CropsGrownSincePollination", 0);
                saved.put_int("TicksSincePollination", 0);
            }
            saved.put_int("CannotEnterHiveTicks", if emergency { 400 } else { 0 });
            let mob = crate::entity::r#type::from_type(
                &EntityType::BEE,
                entrance,
                world,
                uuid::Uuid::new_v4(),
            );
            let mut data = NbtCompound::new();
            mob.write_nbt(&mut data);
            for (key, value) in saved.child_tags {
                data.child_tags.insert(key, value);
            }
            mob.read_nbt_non_mut(&data);
            if let Some(bee) = mob.cast_any().downcast_ref::<BeeEntity>() {
                bee.hive_position.store(Some(self.position));
                if bee.flower_position.load().is_none() {
                    bee.flower_position.store(
                        *self
                            .flower_pos
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner),
                    );
                }
            }
            mob.get_entity().play_sound(Sound::BlockBeehiveExit);
            world.spawn_entity(mob);
            if nectar && !emergency && is_hive {
                let mut props =
                    BeeNestLikeProperties::from_state_id(world.get_block_state_id(&self.position));
                if props.honey_level < 5 {
                    props.honey_level = (props.honey_level
                        + if rand::random_range(0..100) == 0 {
                            2
                        } else {
                            1
                        })
                    .min(5);
                    world.set_block_state(
                        &self.position,
                        props.to_state_id(block),
                        BlockFlags::NOTIFY_ALL,
                    );
                }
            }
        }
        if any {
            self.mark_dirty(world);
        }
        let _ = state;
    }
}
