use crate::{
    db::{lobby::get::get_lobby_info, user::get::get_user_by_id},
    models::{
        game::{LobbyState, Player},
        lobby::{JoinState, LobbyServerMessage},
    },
    state::{ConnectionInfoMap, LobbyJoinRequests, RedisClient},
    ws::handlers::lobby::message_handler::{
        broadcast_to_lobby,
        handler::{get_pending_players, send_error_to_player, send_to_player, set_join_state},
    },
};
use uuid::Uuid;

pub async fn permit_join(
    allow: bool,
    user_id: Uuid,
    join_requests: &LobbyJoinRequests,
    player: Player,
    lobby_id: Uuid,
    connections: &ConnectionInfoMap,
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

    if lobby_info.creator.id != player.id {
        tracing::error!("Unauthorized permit attempt by {}", player.wallet_address);
        send_error_to_player(
            player.id,
            "Only lobby creator can permit joins".to_string(),
            &connections,
            &redis,
        )
        .await;
        return;
    }

    if lobby_info.state != LobbyState::Waiting {
        tracing::error!("Cannot permit joins when game is not waiting");
        send_error_to_player(
            player.id,
            "Cannot permit joins when game is not open".to_string(),
            &connections,
            &redis,
        )
        .await;
        return;
    }

    // Get the user to update their join state
    let user = match get_user_by_id(user_id, redis.clone()).await {
        Ok(user) => user,
        Err(e) => {
            tracing::error!("Failed to fetch user info for {}: {}", user_id, e);
            send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
            return;
        }
    };

    // Use set_join_state instead of accept/reject_join_request
    let new_state = if allow {
        JoinState::Allowed
    } else {
        JoinState::Rejected
    };

    if let Err(e) = set_join_state(lobby_id, user.clone(), new_state.clone(), join_requests).await {
        tracing::error!("Failed to update join state: {}", e);
        send_error_to_player(player.id, e.to_string(), &connections, &redis).await;
        return;
    }

    // Send response to the user who requested to join
    let response_msg = if allow {
        LobbyServerMessage::Allowed
    } else {
        LobbyServerMessage::Rejected
    };
    send_to_player(user_id, &connections, &response_msg, &redis).await;

    // Get updated pending players (which now includes users with all states: Pending, Allowed, Rejected, Idle)
    if let Ok(pending_players) = get_pending_players(lobby_id, &join_requests).await {
        let pending_msg = LobbyServerMessage::PendingPlayers { pending_players };
        broadcast_to_lobby(lobby_id, &pending_msg, &connections, None, redis.clone()).await;
    }

    tracing::info!(
        "Player {} {} user {} for lobby {}",
        player.wallet_address,
        if allow { "allowed" } else { "rejected" },
        user.wallet_address,
        lobby_id
    );
}
