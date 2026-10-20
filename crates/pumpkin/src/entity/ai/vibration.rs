use crate::world::World;
use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::{position::BlockPos, vector3::Vector3};

/// Vibration rays ignore ordinary walls. Every one of the six slightly offset
/// rays must hit a vibration-occluding block for the signal to be blocked.
pub fn is_occluded(world: &World, source: Vector3<f64>, listener: Vector3<f64>) -> bool {
    let source = BlockPos::floored_v(source).to_centered_f64();
    [
        Vector3::new(0.00001, 0.0, 0.0),
        Vector3::new(-0.00001, 0.0, 0.0),
        Vector3::new(0.0, 0.00001, 0.0),
        Vector3::new(0.0, -0.00001, 0.0),
        Vector3::new(0.0, 0.0, 0.00001),
        Vector3::new(0.0, 0.0, -0.00001),
    ]
    .iter()
    .all(|offset| {
        let start = source + *offset;
        let delta = listener - start;
        let steps = (delta.length() * 8.0).ceil().max(1.0) as i32;
        (0..=steps).any(|step| {
            world
                .get_block(&BlockPos::floored_v(
                    start + delta * (f64::from(step) / f64::from(steps)),
                ))
                .has_tag(&tag::Block::MINECRAFT_OCCLUDES_VIBRATION_SIGNALS)
        })
    })
}
