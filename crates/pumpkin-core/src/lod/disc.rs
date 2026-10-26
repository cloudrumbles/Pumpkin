//! The order in which a disc of chunks around a player is visited.

use std::sync::LazyLock;

/// The largest radius a client may request, in chunks.
pub const MAX_RADIUS: i32 = 512;

/// Chunk offsets within [`MAX_RADIUS`] of a centre, nearest first.
///
/// Offsets are grouped in rings of whole-chunk distance: ring `k` holds every
/// offset whose distance from the centre is at least `k` and below `k + 1`.
/// A disc of radius `r` is rings `0..r`, which is a prefix of the order.
/// Within a ring, offsets run around the centre by angle. Consecutive offsets
/// are therefore neighbours, so chunks generated together share the
/// neighbouring chunks their generation depends on.
pub struct DiscOrder {
    offsets: Vec<(i16, i16)>,
    /// `ring_starts[k]` is the index of ring `k`'s first offset; the last
    /// entry is the number of offsets.
    ring_starts: Vec<usize>,
}

static ORDER: LazyLock<DiscOrder> = LazyLock::new(|| DiscOrder::new(MAX_RADIUS));

impl DiscOrder {
    /// Returns the order for [`MAX_RADIUS`], built on first use.
    #[must_use]
    pub fn get() -> &'static Self {
        &ORDER
    }

    fn new(max_radius: i32) -> Self {
        let mut offsets: Vec<(u32, f64, (i16, i16))> = Vec::new();
        for dx in -max_radius..=max_radius {
            for dz in -max_radius..=max_radius {
                let ring = ring_of(dx, dz);
                if ring < max_radius as u32 {
                    let angle = f64::from(dz).atan2(f64::from(dx));
                    offsets.push((ring, angle, (dx as i16, dz as i16)));
                }
            }
        }
        offsets.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));

        let mut ring_starts = Vec::with_capacity(max_radius as usize + 1);
        for (index, &(ring, _, _)) in offsets.iter().enumerate() {
            while ring_starts.len() <= ring as usize {
                ring_starts.push(index);
            }
        }
        ring_starts.push(offsets.len());
        Self {
            offsets: offsets.into_iter().map(|(_, _, offset)| offset).collect(),
            ring_starts,
        }
    }

    /// Returns the offsets of the disc of `radius`, nearest first.
    #[must_use]
    pub fn within(&self, radius: i32) -> &[(i16, i16)] {
        &self.offsets[..self.len_within(radius)]
    }

    /// Returns how many chunks the disc of `radius` holds.
    #[must_use]
    pub fn len_within(&self, radius: i32) -> usize {
        self.ring_starts[radius.clamp(0, self.max_radius()) as usize]
    }

    /// Returns the ring of the offset at `index`.
    #[must_use]
    pub fn ring_at(&self, index: usize) -> u32 {
        let (dx, dz) = self.offsets[index];
        ring_of(i32::from(dx), i32::from(dz))
    }

    const fn max_radius(&self) -> i32 {
        (self.ring_starts.len() - 1) as i32
    }
}

/// Returns the whole-chunk distance of an offset from the centre, rounded
/// down.
#[must_use]
pub fn ring_of(dx: i32, dz: i32) -> u32 {
    let squared = (i64::from(dx) * i64::from(dx) + i64::from(dz) * i64::from(dz)) as u64;
    squared.isqrt() as u32
}

/// Returns whether the chunk at offset `(dx, dz)` lies in the disc of
/// `radius`.
#[must_use]
pub fn is_within(dx: i32, dz: i32, radius: i32) -> bool {
    radius > 0 && ring_of(dx, dz) < radius as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_offsets_come_first() {
        let order = DiscOrder::new(40);
        let offsets = order.within(40);
        assert_eq!(offsets[0], (0, 0));
        for pair in offsets.windows(2) {
            let a = ring_of(i32::from(pair[0].0), i32::from(pair[0].1));
            let b = ring_of(i32::from(pair[1].0), i32::from(pair[1].1));
            assert!(a <= b, "{:?} comes before {:?}", pair[0], pair[1]);
        }
    }

    #[test]
    fn a_disc_holds_every_offset_closer_than_its_radius_once() {
        let order = DiscOrder::new(40);
        for radius in [1, 2, 7, 40] {
            let mut expected = Vec::new();
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    if is_within(dx, dz, radius) {
                        expected.push((dx as i16, dz as i16));
                    }
                }
            }
            let mut actual = order.within(radius).to_vec();
            actual.sort_unstable();
            expected.sort_unstable();
            assert_eq!(actual, expected, "radius {radius}");
        }
        assert_eq!(order.len_within(1), 1);
        assert_eq!(order.len_within(0), 0);
    }

    #[test]
    fn consecutive_offsets_in_a_ring_are_neighbours() {
        let order = DiscOrder::new(64);
        let offsets = order.within(64);
        let ring = 50;
        let start = order.len_within(ring);
        let end = order.len_within(ring + 1);
        for pair in offsets[start..end].windows(2) {
            let step = (i32::from(pair[0].0) - i32::from(pair[1].0))
                .abs()
                .max((i32::from(pair[0].1) - i32::from(pair[1].1)).abs());
            assert!(step <= 2, "{:?} then {:?}", pair[0], pair[1]);
        }
    }

    #[test]
    fn the_full_disc_has_the_size_of_a_circle() {
        let order = DiscOrder::get();
        let area = std::f64::consts::PI * f64::from(MAX_RADIUS).powi(2);
        let len = order.len_within(MAX_RADIUS) as f64;
        assert!((len - area).abs() / area < 0.01, "{len} chunks");
        assert_eq!(order.ring_at(order.len_within(10)), 10);
    }
}
