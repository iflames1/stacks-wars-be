use redis::AsyncCommands;
use uuid::Uuid;

use crate::{errors::AppError, models::game::GameType, state::RedisClient};

pub async fn add_game(mut game: GameType, redis: RedisClient) -> Result<Uuid, AppError> {
    let id = Uuid::new_v4();
    game.id = id;

    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("game:{}", id);
    let value = serde_json::to_string(&game)
        .map_err(|_| AppError::Serialization("Failed to serialize game".to_string()))?;

    let _: () = conn
        .set(key, value)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    Ok(id)
}

pub async fn get_game(game_id: Uuid, redis: RedisClient) -> Result<GameType, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("game:{}", game_id);
    let value: Option<String> = conn
        .get(key)
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    match value {
        Some(data) => serde_json::from_str(&data)
            .map_err(|_| AppError::Deserialization("Failed to deserialize game".to_string())),
        None => Err(AppError::NotFound(format!(
            "Game with ID {} not found",
            game_id
        ))),
    }
}

pub async fn get_all_games(redis: RedisClient) -> Result<Vec<GameType>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let keys: Vec<String> = conn
        .keys("game:*")
        .await
        .map_err(|e| AppError::RedisCommandError(e.into()))?;

    let mut games = Vec::new();
    for key in keys {
        let value: String = conn
            .get(key)
            .await
            .map_err(|e| AppError::RedisCommandError(e.into()))?;

        let game: GameType = serde_json::from_str(&value)
            .map_err(|_| AppError::Deserialization("Failed to deserialize game".to_string()))?;

        games.push(game);
    }

    Ok(games)
}
