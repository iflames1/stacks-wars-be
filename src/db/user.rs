use crate::state::RedisClient;
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
        .arg(format!("wallet:{}", wallet_address))
        .arg(user_id.to_string())
        .query_async(&mut *conn)
        .await
        .unwrap();

    Ok(user_id)
}
