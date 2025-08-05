use crate::{
    db::lobby::{get::get_lobby_players, patch},
    models::{game::Player, lobby::LobbyServerMessage},
    state::{ChatConnectionInfoMap, ConnectionInfoMap, RedisClient},
    ws::handlers::{
        lobby::message_handler::{broadcast_to_lobby, handler::send_error_to_player},
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
    }
    remove_connection(player.id, &connections).await;
}
