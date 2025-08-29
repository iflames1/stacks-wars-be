use redis::AsyncCommands;
use uuid::Uuid;

use crate::{
    db::lobby::get::get_lobby_players,
    errors::AppError,
    models::{
        game::Player,
        lobby::LobbyServerMessage,
        redis::{KeyPart, RedisKey},
    },
    state::{ConnectionInfoMap, RedisClient},
    ws::handlers::lobby::message_handler::broadcast_to_lobby,
};

pub async fn last_ping(
    ts: u64,
    lobby_id: Uuid,
    player: &Player,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    match update_player_last_ping(lobby_id, player.id, ts, redis.clone()).await {
        Ok(()) => {
            if let Ok(players) = get_lobby_players(lobby_id, None, redis.clone()).await {
                let msg = LobbyServerMessage::PlayerUpdated { players };
                broadcast_to_lobby(lobby_id, &msg, connections, None, redis.clone()).await;
            }
        }
        Err(e) => {
            tracing::error!(
                "Failed to update last_ping for player {} in lobby {}: {}",
                player.id,
                lobby_id,
                e
            );
        }
    }
}

async fn update_player_last_ping(
    lobby_id: Uuid,
    player_id: Uuid,
    last_ping: u64,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let player_key = RedisKey::lobby_player(KeyPart::Id(lobby_id), KeyPart::Id(player_id));

    let exists: bool = conn
        .exists(&player_key)
        .await
        .map_err(AppError::RedisCommandError)?;

    if !exists {
        return Err(AppError::NotFound(format!(
            "Player {} not found in lobby {}",
            player_id, lobby_id
        )));
    }

    let _: () = conn
        .hset(&player_key, "last_ping", last_ping.to_string())
        .await
        .map_err(AppError::RedisCommandError)?;

    tracing::debug!(
        "Updated last_ping for player {} in lobby {} to {}",
        player_id,
        lobby_id,
        last_ping
    );

    Ok(())
}
