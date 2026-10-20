#[allow(clippy::wildcard_imports)]
use super::*;
use pumpkin_util::version::JavaMinecraftVersion;

mod state;
use state::{Confirmation, confirm, valid_pose};

impl JavaClient {
    pub fn handle_confirm_teleport(
        &self,
        player: &Arc<Player>,
        server: &Arc<Server>,
        confirm_teleport: &SConfirmTeleport,
    ) {
        let has_accepted_pose = self.version.load() >= JavaMinecraftVersion::V_26_3;
        let position = confirm_teleport.position;
        let result = {
            let mut awaiting_teleport = player
                .awaiting_teleport
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if std::env::var_os("PUMPKIN_TELEPORT_DIAGNOSTICS").is_some_and(|value| value == "1") {
                tracing::info!(
                    target: "pumpkin::teleport",
                    received = confirm_teleport.teleport_id.0,
                    expected = awaiting_teleport.as_ref().map(|(id, _)| id.0),
                    "Teleport confirmation"
                );
            }
            let latest_id = VarInt(player.teleport_id_count.load(Ordering::Relaxed));
            let pose_is_valid = !has_accepted_pose
                || valid_pose(
                    [position.x, position.y, position.z],
                    confirm_teleport.yaw,
                    confirm_teleport.pitch,
                );
            let result = confirm(
                &mut awaiting_teleport,
                &latest_id,
                &confirm_teleport.teleport_id,
                pose_is_valid,
            );
            if let Confirmation::Accepted(position) = result {
                player.get_entity().set_pos(position);
            }
            result
        };

        match result {
            Confirmation::Accepted(_) if has_accepted_pose => {
                // The 26.3 echo is a movement update after acknowledging the
                // server position, including during the initial client load.
                self.handle_player_position_change(
                    player,
                    server,
                    &SPlayerPositionRotation {
                        position,
                        yaw: confirm_teleport.yaw,
                        pitch: confirm_teleport.pitch,
                        collision: 0,
                    },
                );
            }
            Confirmation::Accepted(_) | Confirmation::Stale => {}
            Confirmation::NotTeleporting | Confirmation::InvalidPose => {
                self.try_kick(&TextComponent::translate_cross(
                    translation::java::MULTIPLAYER_DISCONNECT_INVALID_PLAYER_MOVEMENT,
                    translation::java::MULTIPLAYER_DISCONNECT_INVALID_PLAYER_MOVEMENT,
                    [],
                ));
            }
        }
    }
}
