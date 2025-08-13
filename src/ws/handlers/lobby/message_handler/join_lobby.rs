use crate::{
    db::lobby::{get::get_lobby_players, join_requests::get_player_join_request, patch},
    models::{
        game::Player,
        lobby::{JoinState, LobbyServerMessage},
    },
    state::{ChatConnectionInfoMap, ConnectionInfoMap, RedisClient},
    ws::handlers::lobby::message_handler::{broadcast_to_lobby, handler::send_error_to_player},
};
use uuid::Uuid;

pub async fn join_lobby(
    tx_id: Option<String>,
    lobby_id: Uuid,
    player: &Player,
    connections: &ConnectionInfoMap,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    // Check if player has an allowed join request
    match get_player_join_request(lobby_id, player.id, redis.clone()).await {
        Ok(Some(join_request)) => {
            if join_request.state == JoinState::Allowed {
                // Player is allowed to join
                if let Err(e) = patch::join_lobby(lobby_id, player.id, tx_id, redis.clone()).await {
                    tracing::error!("Failed to join lobby: {}", e);
                    send_error_to_player(player.id, lobby_id, e.to_string(), &connections, &redis)
                        .await;
                } else if let Ok(players) = get_lobby_players(lobby_id, None, redis.clone()).await {
                    tracing::info!("{} joined lobby {} successfully", player.id, lobby_id);
                    let msg = LobbyServerMessage::PlayerUpdated { players };
                    broadcast_to_lobby(
                        lobby_id,
                        &msg,
                        &connections,
                        Some(&chat_connections),
                        redis.clone(),
                    )
                    .await;
                }
            } else {
                // Player has a request but it's not allowed
                let message = match join_request.state {
                    JoinState::Pending => "Join request is still pending approval",
                    JoinState::Rejected => "Join request has been rejected",
                    JoinState::Allowed => unreachable!(), // Already handled above
                };

                tracing::warn!(
                    "User {} attempted to join lobby {} with status {:?}",
                    player.id,
                    lobby_id,
                    join_request.state
                );
                send_error_to_player(player.id, lobby_id, message, &connections, &redis).await;
            }
        }
        Ok(None) => {
            // No join request found
            tracing::warn!(
                "User {} attempted to join lobby {} without a join request",
                player.id,
                lobby_id
            );
            send_error_to_player(
                player.id,
                lobby_id,
                "No join request found. Please request to join first.",
                &connections,
                &redis,
            )
            .await;
        }
        Err(e) => {
            // Error getting join request
            tracing::error!(
                "Failed to get join request for player {} in lobby {}: {}",
                player.id,
                lobby_id,
                e
            );
            send_error_to_player(
                player.id,
                lobby_id,
                "Failed to verify join request",
                &connections,
                &redis,
            )
            .await;
        }
    }
}
