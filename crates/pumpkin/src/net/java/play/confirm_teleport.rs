#[allow(clippy::wildcard_imports)]
use super::*;

mod state;
use state::{Confirmation, confirm};

impl JavaClient {
    pub fn handle_confirm_teleport(&self, player: &Player, confirm_teleport: &SConfirmTeleport) {
        let result = {
            let mut awaiting_teleport = player
                .awaiting_teleport
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let result = confirm(&mut awaiting_teleport, &confirm_teleport.teleport_id);
            if let Confirmation::Accepted(position) = result {
                player.get_entity().set_pos(position);
            }
            result
        };

        match result {
            Confirmation::Accepted(_) | Confirmation::Stale => {}
            Confirmation::WrongId => {
                self.try_kick(&TextComponent::text("Wrong teleport id"));
            }
            Confirmation::NotTeleporting => {
                self.try_kick(&TextComponent::text(
                    "Send Teleport confirm, but we did not teleport",
                ));
            }
        }
    }
}
