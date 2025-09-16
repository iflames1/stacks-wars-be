use redis::AsyncCommands;

use crate::{
    errors::AppError,
    models::redis::{KeyPart, RedisKey},
    state::RedisClient,
};

pub async fn clear_all_games(redis: RedisClient) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    // Get all game keys
    let keys: Vec<String> = conn
        .keys(RedisKey::game(KeyPart::Wildcard))
        .await
        .map_err(AppError::RedisCommandError)?;

    if !keys.is_empty() {
        // Delete all game keys
        let _: () = conn
            .del(&keys)
            .await
            .map_err(AppError::RedisCommandError)?;

        tracing::info!("Cleared {} game(s) from database", keys.len());
    }

    Ok(())
}