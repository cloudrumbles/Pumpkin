//! The bound on `lod_chunk` columns sent but not yet acknowledged.

/// Columns one client may have sent but not yet acknowledged.
pub const MAX_UNACKNOWLEDGED: u32 = 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AckError {
    #[error("acknowledged {applied} columns, but only {sent} were sent")]
    AheadOfSent { applied: u32, sent: u32 },
    #[error("acknowledged {applied} columns after already acknowledging {previous}")]
    WentBackwards { applied: u32, previous: u32 },
    #[error("acknowledged a negative number of columns ({applied})")]
    Negative { applied: i32 },
}

/// Counts the columns sent to one client and those it has applied, both since
/// its `lod_request`.
#[derive(Debug, Default)]
pub struct AckWindow {
    sent: u32,
    applied: u32,
}

impl AckWindow {
    /// Returns whether another column may be sent now.
    #[must_use]
    pub const fn has_room(&self) -> bool {
        self.sent - self.applied < MAX_UNACKNOWLEDGED
    }

    /// Records that a column was sent.
    pub const fn record_sent(&mut self) {
        debug_assert!(self.has_room());
        self.sent += 1;
    }

    /// Applies an `lod_ack` reporting `applied` columns applied so far.
    ///
    /// # Errors
    ///
    /// Returns an error if `applied` is negative, below an earlier
    /// acknowledgement or above the columns sent.
    pub fn acknowledge(&mut self, applied: i32) -> Result<(), AckError> {
        let applied = u32::try_from(applied).map_err(|_| AckError::Negative { applied })?;
        if applied > self.sent {
            return Err(AckError::AheadOfSent {
                applied,
                sent: self.sent,
            });
        }
        if applied < self.applied {
            return Err(AckError::WentBackwards {
                applied,
                previous: self.applied,
            });
        }
        self.applied = applied;
        Ok(())
    }

    #[must_use]
    pub const fn applied(&self) -> u32 {
        self.applied
    }

    #[cfg(test)]
    #[must_use]
    pub const fn sent(&self) -> u32 {
        self.sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send_until_full(window: &mut AckWindow) -> u32 {
        let mut count = 0;
        while window.has_room() {
            window.record_sent();
            count += 1;
        }
        count
    }

    #[test]
    fn stops_at_the_unacknowledged_limit() {
        let mut window = AckWindow::default();
        assert_eq!(send_until_full(&mut window), MAX_UNACKNOWLEDGED);
        assert!(!window.has_room());
    }

    #[test]
    fn an_acknowledgement_makes_room_for_as_many_columns() {
        let mut window = AckWindow::default();
        send_until_full(&mut window);
        window.acknowledge(10).unwrap();
        assert_eq!(send_until_full(&mut window), 10);
        let sent = MAX_UNACKNOWLEDGED + 10;
        window.acknowledge(i32::try_from(sent).unwrap()).unwrap();
        assert_eq!(send_until_full(&mut window), MAX_UNACKNOWLEDGED);
        assert_eq!(window.sent(), sent + MAX_UNACKNOWLEDGED);
    }

    #[test]
    fn refuses_impossible_acknowledgements() {
        let mut window = AckWindow::default();
        window.record_sent();
        window.record_sent();
        assert_eq!(
            window.acknowledge(3),
            Err(AckError::AheadOfSent {
                applied: 3,
                sent: 2
            })
        );
        window.acknowledge(2).unwrap();
        assert_eq!(
            window.acknowledge(1),
            Err(AckError::WentBackwards {
                applied: 1,
                previous: 2
            })
        );
        assert_eq!(
            window.acknowledge(-1),
            Err(AckError::Negative { applied: -1 })
        );
    }
}
