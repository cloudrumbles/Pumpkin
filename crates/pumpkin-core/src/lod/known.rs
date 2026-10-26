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

/// Returns the lowest sequence among `chunks` that the client does not have.
pub fn lowest_missing(
    chunks: impl IntoIterator<Item = (ChunkPos, i64)>,
    has: impl Fn(ChunkPos, i64) -> bool,
) -> Option<i64> {
    chunks
        .into_iter()
        .filter(|&(pos, sequence)| !has(pos, sequence))
        .map(|(_, sequence)| sequence)
        .min()
}

/// Returns `complete_through` for a disc: just below the lowest sequence the
/// client lacks there, or `last_sequence`, the newest sequence handed out
/// anywhere, when it lacks none. Any chunk generated later gets a higher one.
#[must_use]
pub fn complete_through(lowest_missing: Option<i64>, last_sequence: i64) -> i64 {
    lowest_missing.map_or(last_sequence, |sequence| sequence - 1)
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
    fn lower_sequences_left_unsent_are_sent_next_session() {
        // Nearest first: the near chunks (sequences 5 and 6) went out and were
        // acknowledged; the far chunk generated first (sequence 2) did not.
        let far = (Vector2::new(30, 0), 2);
        let near = [(Vector2::new(1, 0), 5), (Vector2::new(0, 1), 6)];
        let acknowledged = |pos: ChunkPos, _| near.iter().any(|&(p, _)| p == pos);
        let chunks = [far, near[0], near[1]];

        let through = complete_through(lowest_missing(chunks, acknowledged), 6);
        assert_eq!(through, 1);

        // The client stores that and the disc, and sends them next session.
        let known = KnownDisc::from_request(&request(through, (0, 0), 64));
        assert!(!known.has(far.0, far.1), "the far chunk must be sent");
    }

    #[test]
    fn a_chunk_outside_the_earlier_disc_is_sent_once_it_falls_inside() {
        // Generated for another player far away while this client's disc was
        // centred at the origin, so this client never received it.
        let elsewhere = (Vector2::new(100, 0), 3);
        let inside = (Vector2::new(2, 2), 4);
        let chunks = [inside];
        let through = complete_through(lowest_missing(chunks, |_, _| true), 4);
        assert_eq!(through, 4);

        // The player walks east; the old disc no longer covers the chunk.
        let known = KnownDisc::from_request(&request(through, (0, 0), 64));
        assert!(!known.has(elsewhere.0, elsewhere.1), "it must be sent");
        assert!(known.has(inside.0, inside.1));
    }

    #[test]
    fn a_complete_disc_reaches_the_newest_sequence() {
        assert_eq!(complete_through(None, 41), 41);
        assert_eq!(complete_through(None, -1), -1);
    }
}
