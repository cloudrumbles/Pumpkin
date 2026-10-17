use super::{Controls, Goal, is_valid_attack_target};
use crate::entity::{
    Entity,
    ai::pathfinder::NavigatorGoal,
    mob::Mob,
    projectile::{
        ThrownItemEntity,
        wind_charge::{WIND_CHARGE_GRAVITY, WindChargeEntity},
    },
};
use pumpkin_data::{
    entity::{EntityPose, EntityType},
    sound::{Sound, SoundCategory},
};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};
use std::sync::{Arc, atomic::Ordering};

#[derive(Clone, Copy)]
enum Phase {
    Ready,
    Shooting,
    Recovering,
    Inhaling(Vector3<f64>),
    Jumping,
    Sliding,
}

pub struct BreezeAttackGoal {
    phase: Phase,
    remaining: i32,
    jump_next: bool,
}

impl Default for BreezeAttackGoal {
    fn default() -> Self {
        Self {
            phase: Phase::Ready,
            remaining: 0,
            jump_next: false,
        }
    }
}

impl BreezeAttackGoal {
    fn stop_navigation(mob: &dyn Mob) {
        mob.get_mob_entity()
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop();
        mob.get_mob_entity()
            .living_entity
            .movement_input
            .store(Vector3::new(0.0, 0.0, 0.0));
    }

    fn begin_shot(&mut self, mob: &dyn Mob) {
        Self::stop_navigation(mob);
        mob.get_entity().set_pose(EntityPose::Shooting);
        mob.get_entity().play_sound(Sound::EntityBreezeInhale);
        self.phase = Phase::Shooting;
        self.remaining = 15;
    }

    /// Solve the discrete trajectory using the server's air drag and gravity.
    /// Reject jumps whose swept body intersects terrain before the landing point.
    fn jump_velocity(mob: &dyn Mob, target: Vector3<f64>) -> Option<Vector3<f64>> {
        let entity = mob.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();
        for _ in 0..12 {
            let angle: f64 = rand::random_range(0.0..std::f64::consts::TAU);
            let radius: f64 = rand::random_range(4.0..8.0);
            let x = target.x + angle.cos() * radius;
            let z = target.z + angle.sin() * radius;
            for y in (pos.y.floor() as i32 - 4..=pos.y.floor() as i32 + 4).rev() {
                let floor = BlockPos::floored(x, f64::from(y) - 1.0, z);
                let landing = Vector3::new(x, f64::from(y), z);
                if !world.get_block(&floor).is_solid()
                    || !world.get_block(&floor.up()).is_air()
                    || !world.get_block(&floor.up().up()).is_air()
                {
                    continue;
                }
                let delta = landing - pos;
                if delta.length_squared() < 9.0 {
                    continue;
                }
                let ticks = 20;
                let horizontal_sum = (1.0 - 0.91_f64.powi(ticks)) / 0.09;
                let vertical_sum = (1.0 - 0.98_f64.powi(ticks)) / 0.02;
                let gravity = mob.get_mob_gravity();
                let velocity = Vector3::new(
                    delta.x / horizontal_sum,
                    (delta.y + gravity * 0.98 / 0.02 * (f64::from(ticks) - vertical_sum))
                        / vertical_sum,
                    delta.z / horizontal_sum,
                );
                let mut point = pos;
                let mut step = velocity;
                let mut clear = true;
                for _ in 0..ticks - 1 {
                    point += step;
                    let bounds = pumpkin_util::math::boundingbox::BoundingBox::new(
                        entity.bounding_box.load().min + point - pos,
                        entity.bounding_box.load().max + point - pos,
                    );
                    if !world.is_space_empty(bounds) {
                        clear = false;
                        break;
                    }
                    step = Vector3::new(step.x * 0.91, (step.y - gravity) * 0.98, step.z * 0.91);
                }
                if clear {
                    return Some(velocity);
                }
            }
        }
        None
    }
}

impl Goal for BreezeAttackGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .get_target()
            .is_some_and(|t| is_valid_attack_target(mob, t.as_ref()))
    }
    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob)
    }
    fn start(&mut self, _mob: &dyn Mob) {
        self.phase = Phase::Ready;
        self.remaining = 0;
    }
    fn stop(&mut self, mob: &dyn Mob) {
        Self::stop_navigation(mob);
        mob.get_entity().set_pose(EntityPose::Standing);
        self.phase = Phase::Ready;
    }
    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        if !is_valid_attack_target(mob, target.as_ref()) {
            self.stop(mob);
            return;
        }
        let entity = mob.get_entity();
        let target_pos = target.get_entity().pos.load();
        let distance = entity.pos.load().squared_distance_to_vec(&target_pos);
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_entity_with_range(&target, 30.0, 30.0);
        self.remaining -= 1;
        match self.phase {
            Phase::Ready if self.remaining <= 0 => {
                if self.jump_next
                    && (entity.on_ground.load(Ordering::Relaxed)
                        || entity.touching_water.load(Ordering::Relaxed))
                {
                    self.jump_next = false;
                    if let Some(velocity) = Self::jump_velocity(mob, target_pos) {
                        Self::stop_navigation(mob);
                        entity.set_pose(EntityPose::Inhaling);
                        entity.play_sound(Sound::EntityBreezeCharge);
                        self.phase = Phase::Inhaling(velocity);
                        self.remaining = 10;
                        return;
                    }
                }
                if distance < 256.0 && mob.has_line_of_sight(target.get_entity()) {
                    self.begin_shot(mob);
                } else {
                    mob.get_mob_entity()
                        .navigator
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .set_progress(NavigatorGoal {
                            current_progress: entity.pos.load(),
                            destination: target_pos,
                            speed: 0.6,
                        });
                    self.phase = Phase::Sliding;
                    self.remaining = 40;
                }
            }
            Phase::Shooting if self.remaining <= 0 => {
                if distance < 256.0 && mob.has_line_of_sight(target.get_entity()) {
                    let world = entity.world.load_full();
                    let projectile = ThrownItemEntity::new(
                        Entity::new(
                            world.clone(),
                            entity.pos.load(),
                            &EntityType::BREEZE_WIND_CHARGE,
                        ),
                        entity,
                        WIND_CHARGE_GRAVITY,
                    );
                    let aim = target_pos
                        + Vector3::new(0.0, f64::from(target.get_entity().height()) * 0.3, 0.0)
                        - projectile.entity.pos.load();
                    let difficulty = world.level_info.load().difficulty as i32;
                    projectile.set_velocity(
                        aim.x,
                        aim.y,
                        aim.z,
                        0.7,
                        f64::from((5 - difficulty * 4).max(0)),
                    );
                    world.spawn_entity(Arc::new(WindChargeEntity::new_breeze(projectile)));
                    world.play_sound(
                        Sound::EntityBreezeShoot,
                        SoundCategory::Hostile,
                        &entity.pos.load(),
                    );
                }
                self.phase = Phase::Recovering;
                self.remaining = 4;
            }
            Phase::Recovering if self.remaining <= 0 => {
                entity.set_pose(EntityPose::Standing);
                self.phase = Phase::Ready;
                self.remaining = 10;
                self.jump_next = true;
            }
            Phase::Inhaling(velocity) if self.remaining <= 0 => {
                entity.set_pose(EntityPose::LongJumping);
                entity.on_ground.store(false, Ordering::Relaxed);
                entity.set_velocity(velocity);
                entity.play_sound(Sound::EntityBreezeJump);
                self.phase = Phase::Jumping;
                self.remaining = 60;
            }
            Phase::Jumping
                if self.remaining <= 0
                    || (self.remaining < 57 && entity.on_ground.load(Ordering::Relaxed)) =>
            {
                entity.set_pose(EntityPose::Standing);
                entity.play_sound(Sound::EntityBreezeLand);
                self.phase = Phase::Ready;
                self.remaining = 2;
            }
            Phase::Sliding if self.remaining <= 0 || mob.is_navigator_idle() => {
                self.phase = Phase::Ready;
                self.remaining = 0;
            }
            _ => {}
        }
    }
    fn should_run_every_tick(&self) -> bool {
        true
    }
    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK | Controls::JUMP
    }
}
