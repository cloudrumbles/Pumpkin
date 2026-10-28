//! What a client already has, and how far its acknowledged chunks reach.

use pumpkin_util::math::vector2::Vector2;

use super::disc;
use super::protocol::Request;

type ChunkPos = Vector2<i32>;

/// The chunks a client reported in its `lod_request`: every chunk at or below
/// its `known` sequence inside the disc it had when it acknowledged them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownDisc {
    sequence: i64,
    centre: ChunkPos,
    radius: i32,
}

impl KnownDisc {
    #[must_use]
    pub const fn from_request(request: &Request) -> Self {
        Self {
            sequence: request.known,
            centre: Vector2::new(request.known_centre_x, request.known_centre_z),
            radius: request.known_radius,
        }
    }

    /// Returns whether the client already has the chunk at `pos` as of
    /// `sequence`.
    #[must_use]
    pub fn has(&self, pos: ChunkPos, sequence: i64) -> bool {
        sequence <= self.sequence
            && disc::is_within(pos.x - self.centre.x, pos.y - self.centre.y, self.radius)
    }
}

/// Returns the radius of the largest disc around the centre in which the
/// client has every generated chunk: the ring of the nearest one it lacks, or
/// the whole `radius` when it lacks none. Together with the newest sequence
/// handed out, it tells the client what it can claim next session; chunks
/// generated later inside that disc get higher sequences and are sent.
#[must_use]
pub fn complete_radius(nearest_missing_ring: Option<u32>, radius: i32) -> i32 {
    nearest_missing_ring.map_or(radius, |ring| i32::try_from(ring).unwrap_or(i32::MAX).min(radius))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(known: i64, centre: (i32, i32), radius: i32) -> Request {
        Request {
            radius: 512,
            known,
            known_centre_x: centre.0,
            known_centre_z: centre.1,
            known_radius: radius,
        }
    }

    #[test]
    fn nothing_is_known_without_a_sequence() {
        let known = KnownDisc::from_request(&request(-1, (0, 0), 0));
        assert!(!known.has(Vector2::new(0, 0), 0));
    }

    #[test]
    fn chunks_from_the_nearest_missing_ring_out_are_sent_next_session() {
        // Nearest first: the near chunks went out and were acknowledged; the
        // far one, at ring 30, did not.
        let far = (Vector2::new(30, 0), 2);
        let near = [(Vector2::new(1, 0), 5), (Vector2::new(0, 1), 6)];
        let radius = complete_radius(Some(disc::ring_of(far.0.x, far.0.y)), 64);
        assert_eq!(radius, 30);

        // The client stores that disc with the newest sequence, and sends it
        // next session.
        let known = KnownDisc::from_request(&request(6, (0, 0), radius));
        assert!(!known.has(far.0, far.1), "the far chunk must be sent");
        assert!(near.iter().all(|&(pos, sequence)| known.has(pos, sequence)));
    }

    #[test]
    fn a_chunk_generated_later_inside_the_disc_is_sent() {
        let known = KnownDisc::from_request(&request(6, (0, 0), 30));
        assert!(!known.has(Vector2::new(2, 2), 7));
    }

    #[test]
    fn a_chunk_outside_the_earlier_disc_is_sent_once_it_falls_inside() {
        // Generated for another player far away while this client's disc was
        // centred at the origin, so this client never received it.
        let elsewhere = (Vector2::new(100, 0), 3);
        let inside = (Vector2::new(2, 2), 4);
        let known = KnownDisc::from_request(&request(4, (0, 0), complete_radius(None, 64)));
        assert!(!known.has(elsewhere.0, elsewhere.1), "it must be sent");
        assert!(known.has(inside.0, inside.1));
    }

    #[test]
    fn a_complete_disc_is_the_whole_disc() {
        assert_eq!(complete_radius(None, 512), 512);
        assert_eq!(complete_radius(Some(600), 512), 512);
    }
}
