#[derive(Debug, PartialEq)]
pub(super) enum Confirmation<P> {
    Accepted(P),
    Stale,
    NotTeleporting,
}

pub(super) fn confirm<I: PartialEq, P: Copy>(
    pending: &mut Option<(I, P)>,
    received: &I,
) -> Confirmation<P> {
    match pending.as_ref() {
        // A newer request can replace the pending slot before an earlier ACK
        // arrives. Only the matching ACK may consume the slot or move the player.
        Some((expected, _)) if expected != received => Confirmation::Stale,
        Some(_) => {
            let Some((_, position)) = pending.take() else {
                return Confirmation::NotTeleporting;
            };
            Confirmation::Accepted(position)
        }
        None => Confirmation::NotTeleporting,
    }
}

#[cfg(test)]
mod tests {
    use super::{Confirmation, confirm};

    #[test]
    fn superseded_ack_keeps_newest_request_until_its_ack_arrives() {
        let mut pending = Some((2, [1.0, 64.0, 1.0]));
        assert_eq!(
            pending.replace((3, [2.0, 65.0, 2.0])),
            Some((2, [1.0, 64.0, 1.0]))
        );
        assert_eq!(confirm(&mut pending, &2), Confirmation::Stale);
        assert_eq!(pending, Some((3, [2.0, 65.0, 2.0])));
        assert_eq!(
            confirm(&mut pending, &3),
            Confirmation::Accepted([2.0, 65.0, 2.0])
        );
        assert_eq!(pending, None);
    }

    #[test]
    fn matching_ack_applies_requested_position_once() {
        let mut pending = Some((7, [4, 70, 9]));
        assert_eq!(
            confirm(&mut pending, &7),
            Confirmation::Accepted([4, 70, 9])
        );
        assert_eq!(confirm(&mut pending, &7), Confirmation::NotTeleporting);
    }

    #[test]
    fn unsolicited_ack_is_rejected() {
        let mut pending: Option<(i32, [i32; 3])> = None;
        assert_eq!(confirm(&mut pending, &12), Confirmation::NotTeleporting);
    }
}
