use crate::{models::User, state::RedisClient};
use uuid::Uuid;

pub async fn create_user(
    wallet_address: String,
    redis: RedisClient,
) -> Result<Uuid, redis::RedisError> {
    let user_id = Uuid::new_v4();
    let mut conn = redis.get().await.unwrap();

    let _: () = redis::pipe()
        .cmd("SET")
        .arg(format!("user:{}", user_id))
        .arg(&wallet_address)
        .ignore()
        .cmd("SET")
        .arg(format!("wallet_address:{}", wallet_address))
        .arg(user_id.to_string())
        .query_async(&mut *conn)
        .await
        .unwrap();

    Ok(user_id)
}

pub async fn get_user_by_id(id: Uuid, redis: RedisClient) -> Option<User> {
    let mut conn = redis.get().await.ok()?;
    let key = format!("user:{}", id);

    let user_json: Option<String> = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut *conn)
        .await
        .ok()?;

    user_json.and_then(|json| serde_json::from_str(&json).ok())
}
