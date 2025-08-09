use crate::{
    db::lobby::{get::get_lobby_players, patch},
    models::{game::Player, lobby::LobbyServerMessage},
    state::{ChatConnectionInfoMap, ConnectionInfoMap, LobbyJoinRequests, RedisClient},
    ws::handlers::{
        lobby::message_handler::{
            broadcast_to_lobby,
            handler::{mark_player_as_idle, send_error_to_player, send_to_player},
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
    join_requests: &LobbyJoinRequests,
    redis: &RedisClient,
) {
    if let Err(e) = patch::leave_lobby(lobby_id, player.id, redis.clone()).await {
        tracing::error!("Failed to leave lobby: {}", e);
        send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
    } else if let Ok(players) = get_lobby_players(lobby_id, None, redis.clone()).await {
        tracing::info!("Player {} left lobby {}", player.wallet_address, lobby_id);
        let msg = LobbyServerMessage::PlayerUpdated { players };
        broadcast_to_lobby(
            lobby_id,
            &msg,
            &connections,
            Some(&chat_connections),
            redis.clone(),
        )
        .await;
        send_to_player(player.id, &connections, &msg, redis).await;

        // add to idle
        mark_player_as_idle(lobby_id, player, join_requests, connections, redis).await;
    }
    remove_connection(player.id, &connections).await;
}
