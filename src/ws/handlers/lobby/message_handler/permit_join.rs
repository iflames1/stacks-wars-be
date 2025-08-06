use crate::{
    db::lobby::get::get_lobby_info,
    errors::AppError,
    models::{
        game::Player,
        lobby::{JoinState, LobbyServerMessage},
    },
    state::{ConnectionInfoMap, LobbyJoinRequests, RedisClient},
    ws::handlers::lobby::message_handler::{
        broadcast_to_lobby,
        handler::{get_pending_players, send_error_to_player, send_to_player},
    },
};
use uuid::Uuid;

async fn accept_join_request(
    lobby_id: Uuid,
    user_id: Uuid,
    join_requests: &LobbyJoinRequests,
) -> Result<(), AppError> {
    let mut map = join_requests.lock().await;

    if let Some(requests) = map.get_mut(&lobby_id) {
        tracing::info!("Join requests for lobby {}: {:?}", lobby_id, requests);

        if let Some(req) = requests.iter_mut().find(|r| r.user.id == user_id) {
            tracing::info!("Found join request for user {}", user_id);
            req.state = JoinState::Allowed;
            return Ok(());
        } else {
            tracing::warn!("User {} not found in join requests", user_id);
        }
    } else {
        tracing::warn!("No join requests found for lobby {}", lobby_id);
    }

    Err(AppError::NotFound("User not found in join requests".into()))
}

async fn reject_join_request(
    lobby_id: Uuid,
    user_id: Uuid,
    join_requests: &LobbyJoinRequests,
) -> Result<(), AppError> {
    let mut map = join_requests.lock().await;

    if let Some(requests) = map.get_mut(&lobby_id) {
        tracing::info!("Join requests for lobby {}: {:?}", lobby_id, requests);

        if let Some(req) = requests.iter_mut().find(|r| r.user.id == user_id) {
            tracing::info!("Found join request for user {}", user_id);
            req.state = JoinState::Rejected;
            return Ok(());
        } else {
            tracing::warn!("User {} not found in join requests", user_id);
        }
    } else {
        tracing::warn!("No join requests found for lobby {}", lobby_id);
    }

    Err(AppError::NotFound("User not found in join requests".into()))
}

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
        tracing::warn!(
            "Unauthorized PermitJoin attempt by {}",
            player.wallet_address
        );
        send_error_to_player(
            player.id,
            "Only creator can accept request",
            &connections,
            &redis,
        )
        .await;
        return;
    }

    let result = if allow {
        accept_join_request(lobby_id, user_id, &join_requests).await
    } else {
        reject_join_request(lobby_id, user_id, &join_requests).await
    };

    match result {
        Ok(_) => {
            let msg = if allow {
                LobbyServerMessage::Allowed
            } else {
                LobbyServerMessage::Rejected
            };
            send_to_player(user_id, &connections, &msg, &redis).await;
        }
        Err(e) => {
            tracing::error!("Failed to update join state: {}", e);
            send_error_to_player(user_id, e.to_string(), &connections, &redis).await;
        }
    }

    if let Ok(pending_players) = get_pending_players(lobby_id, &join_requests).await {
        tracing::info!(
            "Updated pending players for lobby {}: {}",
            lobby_id,
            pending_players.len()
        );
        let msg = LobbyServerMessage::PendingPlayers { pending_players };
        broadcast_to_lobby(lobby_id, &msg, &connections, None, redis.clone()).await;
    }
}
