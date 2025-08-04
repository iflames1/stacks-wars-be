use crate::{
    db,
    models::{
        game::{GameState, Player},
        lobby::LobbyServerMessage,
    },
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

pub async fn kick_player(
    player_id: Uuid,
    wallet_address: String,
    display_name: Option<String>,
    room_id: Uuid,
    player: &Player,
    connections: &ConnectionInfoMap,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    let room_info = match db::lobby::get_room_info(room_id, redis.clone()).await {
        Ok(info) => info,
        Err(e) => {
            tracing::error!("Failed to fetch room info: {}", e);
            send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
            return;
        }
    };

    if room_info.creator_id != player.id {
        tracing::error!("Unauthorized kick attempt by {}", player.wallet_address);
        send_error_to_player(
            player.id,
            "Only creator can kick players",
            &connections,
            &redis,
        )
        .await;
        return;
    }

    if room_info.state != GameState::Waiting {
        tracing::error!("Cannot kick players when game is not waiting");
        send_error_to_player(
            player.id,
            "Cannot kick player when game is in progress",
            &connections,
            &redis,
        )
        .await;
        return;
    }

    // Remove player
    if let Err(e) = db::lobby::leave_room(room_id, player_id, redis.clone()).await {
        tracing::error!("Failed to kick player: {}", e);
        send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
    } else if let Ok(players) = db::lobby::get_room_players(room_id, redis.clone()).await {
        let msg = LobbyServerMessage::PlayerUpdated { players };
        broadcast_to_lobby(
            room_id,
            &msg,
            &connections,
            Some(&chat_connections),
            redis.clone(),
        )
        .await;

        tracing::info!("Success kicking {} from {}", player.wallet_address, room_id);
        let kicked_msg = LobbyServerMessage::PlayerKicked {
            player_id,
            wallet_address,
            display_name,
        };
        broadcast_to_lobby(room_id, &kicked_msg, &connections, None, redis.clone()).await;

        let msg: LobbyServerMessage = LobbyServerMessage::NotifyKicked;

        send_to_player(player_id, &connections, &msg, &redis).await;
    }

    remove_connection(player_id, &connections).await;
}
