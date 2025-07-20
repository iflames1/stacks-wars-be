use redis::AsyncCommands;
use uuid::Uuid;

use crate::{errors::AppError, models::game::GameType, state::RedisClient};

pub async fn add_game(game: GameType, redis: RedisClient) -> Result<Uuid, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("game:{}:data", game.id);
    let value = serde_json::to_string(&game)
        .map_err(|_| AppError::Serialization("Failed to serialize game".to_string()))?;

    let _: () = conn
        .set(key, value)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    Ok(game.id)
}
