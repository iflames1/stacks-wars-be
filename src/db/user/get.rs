use crate::{
    errors::AppError,
    models::{User, redis::RedisKey},
    state::RedisClient,
};
use redis::AsyncCommands;
use std::collections::HashMap;
use uuid::Uuid;

pub async fn get_user_by_id(user_id: Uuid, redis: RedisClient) -> Result<User, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = RedisKey::user(user_id);

    let data: HashMap<String, String> = conn
        .hgetall(&key)
        .await
        .map_err(AppError::RedisCommandError)?;

    if data.is_empty() {
        return Err(AppError::NotFound("User not found".into()));
    }

    let user = User {
        id: user_id,
        wallet_address: data
            .get("wallet_address")
            .cloned()
            .unwrap_or_else(|| "".into()),
        display_name: data.get("display_name").cloned(),
        wars_point: data
            .get("wars_point")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0),
        username: data.get("username").cloned(),
    };

    Ok(user)
}

pub async fn get_user_id(identifier: &str, redis: RedisClient) -> Result<Uuid, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    // Try wallet lookup first
    let wallet_key = RedisKey::wallet(identifier);
    if let Ok(Some(user_id_str)) = conn.get::<_, Option<String>>(&wallet_key).await {
        return Uuid::parse_str(&user_id_str)
            .map_err(|e| AppError::Deserialization(format!("Invalid UUID: {}", e)));
    }

    // Fallback to username
    let username_key = RedisKey::username(identifier);
    let user_id_str: String = conn
        .get(&username_key)
        .await
        .map_err(AppError::RedisCommandError)?;

    let user_id = Uuid::parse_str(&user_id_str)
        .map_err(|e| AppError::Deserialization(format!("Invalid UUID: {}", e)))?;

    Ok(user_id)
}
