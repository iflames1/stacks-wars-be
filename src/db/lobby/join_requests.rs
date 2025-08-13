use redis::AsyncCommands;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    errors::AppError,
    models::{
        User,
        lobby::JoinState,
        redis::{KeyPart, RedisKey},
    },
    state::RedisClient,
};

#[derive(Debug, Clone)]
pub struct JoinRequestEntry {
    pub user: User,
    pub state: JoinState,
}

impl JoinRequestEntry {
    pub fn to_redis_hash(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert("state".into(), format!("{:?}", self.state));
        // Store user data for quick access
        map.insert("user_id".into(), self.user.id.to_string());
        map.insert("wallet_address".into(), self.user.wallet_address.clone());
        if let Some(ref username) = self.user.username {
            map.insert("username".into(), username.clone());
        }
        if let Some(ref display_name) = self.user.display_name {
            map.insert("display_name".into(), display_name.clone());
        }
        map.insert("wars_point".into(), self.user.wars_point.to_string());
        map
    }

    pub fn from_redis_hash(
        user_id: Uuid,
        data: &HashMap<String, String>,
    ) -> Result<Self, AppError> {
        let state_str = data
            .get("state")
            .ok_or_else(|| AppError::Deserialization("Missing state".into()))?;

        let state = match state_str.as_str() {
            "Pending" => JoinState::Pending,
            "Allowed" => JoinState::Allowed,
            "Rejected" => JoinState::Rejected,
            other => {
                return Err(AppError::Deserialization(format!(
                    "Invalid join state: {}",
                    other
                )));
            }
        };

        let user = User {
            id: user_id,
            wallet_address: data.get("wallet_address").cloned().unwrap_or_default(),
            username: data.get("username").cloned(),
            display_name: data.get("display_name").cloned(),
            wars_point: data
                .get("wars_point")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0),
        };

        Ok(JoinRequestEntry { user, state })
    }
}

pub async fn get_lobby_join_requests(
    lobby_id: Uuid,
    state_filter: Option<JoinState>,
    redis: RedisClient,
) -> Result<Vec<JoinRequestEntry>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = RedisKey::lobby_join_requests(KeyPart::Id(lobby_id));

    // Get all user IDs in the hash
    let user_ids: Vec<String> = conn
        .hkeys(&key)
        .await
        .map_err(AppError::RedisCommandError)?;

    if user_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut requests = Vec::new();

    for user_id_str in user_ids {
        if let Ok(user_id) = Uuid::parse_str(&user_id_str) {
            // Get the hash for this user
            let user_data: HashMap<String, String> = conn
                .hgetall(format!("{}:{}", key, user_id_str))
                .await
                .map_err(AppError::RedisCommandError)?;

            if !user_data.is_empty() {
                if let Ok(entry) = JoinRequestEntry::from_redis_hash(user_id, &user_data) {
                    // Apply state filter if provided
                    let passes_filter = match &state_filter {
                        Some(filter_state) => entry.state == *filter_state,
                        None => true,
                    };

                    if passes_filter {
                        requests.push(entry);
                    }
                }
            }
        }
    }

    Ok(requests)
}

pub async fn get_player_join_request(
    lobby_id: Uuid,
    user_id: Uuid,
    redis: RedisClient,
) -> Result<Option<JoinRequestEntry>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!(
        "{}:{}",
        RedisKey::lobby_join_requests(KeyPart::Id(lobby_id)),
        user_id
    );

    let user_data: HashMap<String, String> = conn
        .hgetall(&key)
        .await
        .map_err(AppError::RedisCommandError)?;

    if user_data.is_empty() {
        return Ok(None);
    }

    let entry = JoinRequestEntry::from_redis_hash(user_id, &user_data)?;
    Ok(Some(entry))
}

pub async fn update_join_request(
    lobby_id: Uuid,
    user: User,
    new_state: JoinState,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let base_key = RedisKey::lobby_join_requests(KeyPart::Id(lobby_id));
    let user_key = format!("{}:{}", base_key, user.id);

    // Add user_id to the main hash (for easy listing)
    let _: () = conn
        .hset(&base_key, user.id.to_string(), "1")
        .await
        .map_err(AppError::RedisCommandError)?;

    // Create entry with new state
    let entry = JoinRequestEntry {
        user,
        state: new_state,
    };

    let entry_data = entry.to_redis_hash();
    let fields: Vec<(&str, &str)> = entry_data
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    // Store the full entry data
    let _: () = conn
        .hset_multiple(&user_key, &fields)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(())
}

pub async fn remove_join_request(
    lobby_id: Uuid,
    user_id: Uuid,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let base_key = RedisKey::lobby_join_requests(KeyPart::Id(lobby_id));
    let user_key = format!("{}:{}", base_key, user_id);

    // Remove from main hash
    let _: () = conn
        .hdel(&base_key, user_id.to_string())
        .await
        .map_err(AppError::RedisCommandError)?;

    // Remove the user's entry hash
    let _: () = conn
        .del(&user_key)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(())
}
