use crate::{
    db::lobby::{
        get::{get_lobby_info, get_lobby_players},
        patch::leave_lobby,
    },
    models::{
        game::{LobbyState, Player},
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
    lobby_id: Uuid,
    player: &Player,
    connections: &ConnectionInfoMap,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    let lobby_info = match get_lobby_info(lobby_id, redis.clone()).await {
        Ok(info) => info,
        Err(e) => {
            tracing::error!("Failed to fetch lobby info: {}", e);
            send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
            return;
        }
    };

    if lobby_info.creator_id != player.id {
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

    if lobby_info.state != LobbyState::Waiting {
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
    if let Err(e) = leave_lobby(lobby_id, player_id, redis.clone()).await {
        tracing::error!("Failed to kick player: {}", e);
        send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
    } else if let Ok(players) = get_lobby_players(lobby_id, None, redis.clone()).await {
        let msg = LobbyServerMessage::PlayerUpdated { players };
        broadcast_to_lobby(
            lobby_id,
            &msg,
            &connections,
            Some(&chat_connections),
            redis.clone(),
        )
        .await;

        tracing::info!(
            "Success kicking {} from {}",
            player.wallet_address,
            lobby_id
        );
        let kicked_msg = LobbyServerMessage::PlayerKicked {
            player_id,
            wallet_address,
            display_name,
        };
        broadcast_to_lobby(lobby_id, &kicked_msg, &connections, None, redis.clone()).await;

        let msg: LobbyServerMessage = LobbyServerMessage::NotifyKicked;

        send_to_player(player_id, &connections, &msg, &redis).await;
    }

    remove_connection(player_id, &connections).await;
}
