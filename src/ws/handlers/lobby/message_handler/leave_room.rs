use crate::{
    db,
    models::{game::Player, lobby::LobbyServerMessage},
    state::{ChatConnectionInfoMap, ConnectionInfoMap, RedisClient},
    ws::handlers::{
        lobby::message_handler::{broadcast_to_lobby, handler::send_error_to_player},
        utils::remove_connection,
    },
};
use uuid::Uuid;

pub async fn leave_room(
    room_id: Uuid,
    player: &Player,
    connections: &ConnectionInfoMap,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    if let Err(e) = db::room::leave_room(room_id, player.id, redis.clone()).await {
        tracing::error!("Failed to leave room: {}", e);
        send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
    } else if let Ok(players) = db::room::get_room_players(room_id, redis.clone()).await {
        tracing::info!("Player {} left room {}", player.wallet_address, room_id);
        let msg = LobbyServerMessage::PlayerUpdated { players };
        broadcast_to_lobby(
            room_id,
            &msg,
            &connections,
            Some(&chat_connections),
            redis.clone(),
        )
        .await;
    }
    remove_connection(player.id, &connections).await;
}
