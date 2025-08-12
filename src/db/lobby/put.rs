use redis::AsyncCommands;
use uuid::Uuid;

use crate::{
    errors::AppError,
    models::redis::{KeyPart, RedisKey},
    state::RedisClient,
};

pub async fn create_connected_players(
    lobby_id: Uuid,
    connected_player_ids: Vec<Uuid>,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let connected_key = RedisKey::lobby_connected_players(KeyPart::Id(lobby_id));

    let _: () = conn
        .del(&connected_key)
        .await
        .map_err(AppError::RedisCommandError)?;

    // Add all connected player IDs to the set
    if !connected_player_ids.is_empty() {
        let player_id_strings: Vec<String> = connected_player_ids
            .into_iter()
            .map(|id| id.to_string())
            .collect();

        let _: () = conn
            .sadd(&connected_key, player_id_strings)
            .await
            .map_err(AppError::RedisCommandError)?;
    }

    Ok(())
}
