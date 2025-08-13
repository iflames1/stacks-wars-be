use crate::{
    db::{
        lobby::{get::get_lobby_players, join_requests::remove_join_request, patch},
        user::patch::decrease_wars_point,
    },
    models::{game::Player, lobby::LobbyServerMessage},
    state::{ChatConnectionInfoMap, ConnectionInfoMap, RedisClient},
    ws::handlers::{
        lobby::message_handler::{
            broadcast_to_lobby,
            handler::{send_error_to_player, send_to_player},
        },
        utils::remove_connection,
    },
};
use uuid::Uuid;

pub async fn leave_lobby(
    lobby_id: Uuid,
    player: &Player,
    connections: &ConnectionInfoMap,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    if let Err(e) = patch::leave_lobby(lobby_id, player.id, redis.clone()).await {
        tracing::error!("Failed to leave lobby: {}", e);
        send_error_to_player(player.id, lobby_id, e.to_string(), &connections, &redis).await;
    } else if let Ok(players) = get_lobby_players(lobby_id, None, redis.clone()).await {
        tracing::info!("Player {} left lobby {}", player.id, lobby_id);

        // Remove join request when leaving
        if let Err(e) = remove_join_request(lobby_id, player.id, redis.clone()).await {
            tracing::warn!(
                "Failed to remove join request for player {}: {}",
                player.id,
                e
            );
        }

        // Subtract 10 wars points for leaving the lobby
        match decrease_wars_point(player.id, 10.0, redis.clone()).await {
            Ok(new_total) => {
                tracing::info!(
                    "Subtracted 10 wars points from player {} for leaving lobby. New total: {}",
                    player.id,
                    new_total
                );

                let wars_point_msg = LobbyServerMessage::WarsPointDeduction {
                    amount: 10.0,
                    new_total,
                    reason: "Left lobby".to_string(),
                };
                send_to_player(player.id, lobby_id, connections, &wars_point_msg, redis).await;
            }
            Err(e) => {
                tracing::error!(
                    "Failed to subtract wars points from player {} for leaving lobby: {}",
                    player.id,
                    e
                );
            }
        }

        let msg = LobbyServerMessage::PlayerUpdated { players };
        broadcast_to_lobby(
            lobby_id,
            &msg,
            &connections,
            Some(&chat_connections),
            redis.clone(),
        )
        .await;
        send_to_player(player.id, lobby_id, &connections, &msg, redis).await;
    }
    remove_connection(player.id, &connections).await;
}
