use crate::{
    db::lobby::{get::get_lobby_players, patch},
    models::{
        game::Player,
        lobby::{JoinState, LobbyServerMessage},
    },
    state::{ChatConnectionInfoMap, ConnectionInfoMap, LobbyJoinRequests, RedisClient},
    ws::handlers::lobby::message_handler::{
        broadcast_to_lobby,
        handler::{get_join_requests, send_error_to_player},
    },
};
use uuid::Uuid;

pub async fn join_lobby(
    tx_id: Option<String>,
    lobby_id: Uuid,
    join_requests: &LobbyJoinRequests,
    player: &Player,
    connections: &ConnectionInfoMap,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    let join_map = get_join_requests(lobby_id, &join_requests).await;
    if let Some(req) = join_map.iter().find(|r| r.user.id == player.id) {
        if req.state == JoinState::Allowed {
            if let Err(e) = patch::join_lobby(lobby_id, player.id, tx_id, redis.clone()).await {
                tracing::error!("Failed to join lobby: {}", e);
                send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
            } else if let Ok(players) = get_lobby_players(lobby_id, None, redis.clone()).await {
                tracing::info!(
                    "{} joined lobby {} successfully",
                    player.wallet_address,
                    lobby_id
                );
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
