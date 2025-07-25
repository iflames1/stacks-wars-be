use crate::{
    models::{game::Player, lobby::LobbyServerMessage},
    state::{ConnectionInfoMap, LobbyJoinRequests, RedisClient},
    ws::handlers::lobby::message_handler::{
        broadcast_to_lobby,
        handler::{get_pending_players, request_to_join, send_error_to_player, send_to_player},
    },
};
use uuid::Uuid;

pub async fn request_join(
    player: &Player,
    room_id: Uuid,
    join_requests: &LobbyJoinRequests,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    match request_to_join(room_id, player.clone().into(), &join_requests).await {
        Ok(_) => {
            if let Ok(pending_players) = get_pending_players(room_id, &join_requests).await {
                tracing::info!(
                    "Success Adding {} to pending players",
                    player.wallet_address
                );
                let msg = LobbyServerMessage::Pending;
                send_to_player(player.id, &connections, &msg, &redis).await;

                let msg = LobbyServerMessage::PendingPlayers { pending_players };
                broadcast_to_lobby(room_id, &msg, &connections, None, redis.clone()).await;
            }
        }
        Err(e) => {
            tracing::error!("Failed to mark user as pending: {}", e);
            send_error_to_player(
                player.id,
                "Failed to send join request",
                &connections,
                &redis,
            )
            .await;
        }
    }
}
