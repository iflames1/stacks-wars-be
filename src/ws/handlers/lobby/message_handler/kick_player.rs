use crate::{
    db::{
        lobby::{
            get::{get_lobby_info, get_lobby_players},
            patch::leave_lobby,
        },
        user::get::get_user_by_id,
    },
    models::{
        game::{LobbyState, Player},
        lobby::LobbyServerMessage,
    },
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

pub async fn kick_player(
    player_id: Uuid,
    lobby_id: Uuid,
    player: &Player,
    connections: &ConnectionInfoMap,
    chat_connections: &ChatConnectionInfoMap,
    join_requests: &LobbyJoinRequests,
    redis: &RedisClient,
) {
    let lobby_info = match get_lobby_info(lobby_id, redis.clone()).await {
        Ok(info) => info,
        Err(e) => {
            tracing::error!("Failed to fetch lobby info: {}", e);
            send_error_to_player(player.id, lobby_id, e.to_string(), &connections, &redis).await;
            return;
        }
    };

    if lobby_info.creator.id != player.id {
        tracing::error!("Unauthorized kick attempt by {}", player.wallet_address);
        send_error_to_player(
            player.id,
            lobby_id,
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
            lobby_id,
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
        send_error_to_player(player.id, lobby_id, e.to_string(), &connections, &redis).await;
    } else if let Ok(players) = get_lobby_players(lobby_id, None, redis.clone()).await {
        let player_updated_msg = LobbyServerMessage::PlayerUpdated { players };
        broadcast_to_lobby(
            lobby_id,
            &player_updated_msg,
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
        let kicked_user = match get_user_by_id(player_id, redis.clone()).await {
            Ok(user) => user,
            Err(e) => {
                tracing::error!("Failed to fetch player info: {}", e);
                send_error_to_player(player.id, lobby_id, e.to_string(), &connections, &redis)
                    .await;
                return;
            }
        };

        // Create a Player struct for the kicked user to mark as idle
        let kicked_player = Player {
            id: kicked_user.id,
            wallet_address: kicked_user.wallet_address.clone(),
            display_name: kicked_user.display_name.clone(),
            username: kicked_user.username.clone(),
            wars_point: kicked_user.wars_point,
            state: crate::models::game::PlayerState::NotReady,
            used_words: None,
            rank: None,
            tx_id: None,
            claim: None,
            prize: None,
        };

        let kicked_msg = LobbyServerMessage::PlayerKicked {
            player: kicked_user,
        };
        broadcast_to_lobby(lobby_id, &kicked_msg, &connections, None, redis.clone()).await;

        let notify_kicked_msg = LobbyServerMessage::NotifyKicked;
        send_to_player(
            player_id,
            lobby_id,
            &connections,
            &notify_kicked_msg,
            &redis,
        )
        .await;
        send_to_player(
            player_id,
            lobby_id,
            &connections,
            &player_updated_msg,
            &redis,
        )
        .await;

        mark_player_as_idle(lobby_id, &kicked_player, join_requests, connections, redis).await;
    }

    remove_connection(player_id, &connections).await;
}
