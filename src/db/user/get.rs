use crate::{
    errors::AppError,
    models::{
        User,
        redis::{KeyPart, RedisKey},
    },
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

    let key = RedisKey::user(KeyPart::Id(user_id));

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
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0),
        username: data.get("username").cloned(),
    };

    Ok(user)
}

pub async fn _get_user_id(identifier: String, redis: RedisClient) -> Result<Uuid, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    // Try wallet lookup first
    let wallet_key = RedisKey::wallet(KeyPart::Str(identifier.clone()));
    if let Ok(Some(user_id_str)) = conn.get::<_, Option<String>>(&wallet_key).await {
        return Uuid::parse_str(&user_id_str)
            .map_err(|e| AppError::Deserialization(format!("Invalid UUID: {}", e)));
    }

    // Fallback to username
    let username_key = RedisKey::username(KeyPart::Str(identifier));
    let user_id_str: String = conn
        .get(&username_key)
        .await
        .map_err(AppError::RedisCommandError)?;

    let user_id = Uuid::parse_str(&user_id_str)
        .map_err(|e| AppError::Deserialization(format!("Invalid UUID: {}", e)))?;

    Ok(user_id)
}

pub async fn get_all_users(redis: RedisClient) -> Result<Vec<User>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    // Get all user keys
    let pattern = RedisKey::user(KeyPart::Wildcard);
    let user_keys: Vec<String> = redis::cmd("KEYS")
        .arg(pattern)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    let mut users = Vec::new();

    for key in user_keys {
        // Extract user ID from key (users:{uuid})
        if let Some(user_id_str) = key.strip_prefix("users:") {
            if let Ok(user_id) = Uuid::parse_str(user_id_str) {
                if let Ok(user) = get_user_by_id(user_id, redis.clone()).await {
                    users.push(user);
                }
            }
        }
    }

    Ok(users)
}
