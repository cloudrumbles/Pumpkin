#[derive(Debug, PartialEq)]
pub(super) enum Confirmation<P> {
    Accepted(P),
    Stale,
    NotTeleporting,
    InvalidPose,
}

pub(super) fn valid_pose(position: [f64; 3], yaw: f32, pitch: f32) -> bool {
    // Vanilla clamps infinite coordinates, but rejects NaN coordinates and
    // any non-finite rotation.
    !position.into_iter().any(f64::is_nan) && yaw.is_finite() && pitch.is_finite()
}

pub(super) fn confirm<I: PartialEq, P: Copy>(
    pending: &mut Option<(I, P)>,
    latest_id: &I,
    received: &I,
    pose_is_valid: bool,
) -> Confirmation<P> {
    match pending.as_ref() {
        // A superseded ACK must not consume the current request, even when its
        // accepted pose is invalid or the current request is already complete.
        Some((expected, _)) if expected != received => Confirmation::Stale,
        None if latest_id != received => Confirmation::Stale,
        None => Confirmation::NotTeleporting,
        Some(_) if !pose_is_valid => Confirmation::InvalidPose,
        Some(_) => {
            let Some((_, position)) = pending.take() else {
                return Confirmation::NotTeleporting;
            };
            Confirmation::Accepted(position)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Confirmation, confirm, valid_pose};

    #[test]
    fn superseded_ack_keeps_newest_request_until_its_ack_arrives() {
        let mut pending = Some((3, [2.0, 65.0, 2.0]));
        assert_eq!(confirm(&mut pending, &3, &2, true), Confirmation::Stale);
        assert_eq!(pending, Some((3, [2.0, 65.0, 2.0])));
        assert_eq!(
            confirm(&mut pending, &3, &3, true),
            Confirmation::Accepted([2.0, 65.0, 2.0])
        );
        assert_eq!(pending, None);
    }

    #[test]
    fn matching_ack_applies_requested_position_once() {
        let mut pending = Some((7, [4, 70, 9]));
        assert_eq!(
            confirm(&mut pending, &7, &7, true),
            Confirmation::Accepted([4, 70, 9])
        );
        assert_eq!(
            confirm(&mut pending, &7, &7, true),
            Confirmation::NotTeleporting
        );
    }

    #[test]
    fn stale_ack_after_completion_is_ignored() {
        let mut pending: Option<(i32, [i32; 3])> = None;
        assert_eq!(confirm(&mut pending, &7, &6, true), Confirmation::Stale);
    }

    #[test]
    fn invalid_stale_pose_is_ignored_before_validation() {
        let mut pending = Some((7, [4, 70, 9]));
        assert_eq!(confirm(&mut pending, &7, &6, false), Confirmation::Stale);
        assert_eq!(pending, Some((7, [4, 70, 9])));
    }

    #[test]
    fn invalid_matching_pose_does_not_consume_request() {
        let mut pending = Some((7, [4, 70, 9]));
        assert_eq!(
            confirm(&mut pending, &7, &7, false),
            Confirmation::InvalidPose
        );
        assert_eq!(pending, Some((7, [4, 70, 9])));
    }

    #[test]
    fn latest_ack_without_request_is_rejected() {
        let mut pending: Option<(i32, [i32; 3])> = None;
        assert_eq!(
            confirm(&mut pending, &12, &12, true),
            Confirmation::NotTeleporting
        );
    }

    #[test]
    fn coordinates_reject_nan_but_allow_values_for_vanilla_clamping() {
        assert!(valid_pose([1.0, 64.0, 1.0], 0.0, 0.0));
        assert!(valid_pose(
            [f64::INFINITY, f64::NEG_INFINITY, 1.0],
            0.0,
            0.0
        ));
        for index in 0..3 {
            let mut position = [1.0, 64.0, 1.0];
            position[index] = f64::NAN;
            assert!(!valid_pose(position, 0.0, 0.0));
        }
    }

    #[test]
    fn rotations_must_be_finite() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(!valid_pose([1.0, 64.0, 1.0], value, 0.0));
            assert!(!valid_pose([1.0, 64.0, 1.0], 0.0, value));
        }
    }
}
