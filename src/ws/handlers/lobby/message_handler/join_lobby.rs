use crate::{
    db,
    models::{
        game::{LobbyServerMessage, Player},
        lobby::JoinState,
    },
    state::{ConnectionInfoMap, LobbyJoinRequests, RedisClient},
    ws::handlers::lobby::message_handler::{
        broadcast_to_lobby,
        handler::{get_join_requests, send_error_to_player},
    },
};
use uuid::Uuid;

pub async fn join_lobby(
    tx_id: Option<String>,
    room_id: Uuid,
    join_requests: &LobbyJoinRequests,
    player: &Player,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    let join_map = get_join_requests(room_id, &join_requests).await;
    if let Some(req) = join_map.iter().find(|r| r.user.id == player.id) {
        if req.state == JoinState::Allowed {
            if let Err(e) = db::room::join_room(room_id, player.id, tx_id, redis.clone()).await {
                tracing::error!("Failed to join room: {}", e);
                send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
            } else if let Ok(players) = db::room::get_room_players(room_id, redis.clone()).await {
                tracing::info!(
                    "{} joined room {} successfully",
                    player.wallet_address,
                    room_id
                );
                let msg = LobbyServerMessage::PlayerJoined { players };
                broadcast_to_lobby(room_id, &msg, &connections, redis.clone()).await;
            }
        } else {
            tracing::warn!(
                "User {} attempted to join without being allowed",
                player.wallet_address
            );
            send_error_to_player(
                player.id,
                "Join request has to be accpeted to join lobby",
                &connections,
                &redis,
            )
            .await;
        }
    }
}
