use std::collections::HashMap;

use crate::{
    auth::{generate_jwt, generate_jwt_v2},
    db::{season::get::get_current_season, user::get::_get_all_users},
    errors::AppError,
    models::{
        User,
        redis::{KeyPart, RedisKey},
        user::{UserV2, UserWarsPoints},
    },
    state::RedisClient,
};
use redis::AsyncCommands;
use sqlx::PgPool;
use uuid::Uuid;

pub async fn _create_user(wallet_address: String, redis: RedisClient) -> Result<String, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let wallets_hash = RedisKey::users_wallets();

    // Check if wallet is already registered using hash lookup
    if let Some(existing_id) = conn
        .hget::<_, _, Option<String>>(&wallets_hash, &wallet_address)
        .await
        .map_err(AppError::RedisCommandError)?
    {
        let user_id: Uuid = existing_id
            .parse()
            .map_err(|_| AppError::BadRequest("Invalid UUID".into()))?;
        let user_key = RedisKey::user(KeyPart::Id(user_id));
        let user_data: HashMap<String, String> = conn
            .hgetall(&user_key)
            .await
            .map_err(AppError::RedisCommandError)?;
        if user_data.is_empty() {
            return Err(AppError::BadRequest(
                "Wallet mapped but user data not found".into(),
            ));
        }

        let user = User {
            id: existing_id
                .parse()
                .map_err(|_| AppError::BadRequest("Invalid UUID".into()))?,
            wallet_address: user_data.get("wallet_address").cloned().unwrap_or_default(),
            username: user_data.get("username").cloned(),
            display_name: user_data.get("display_name").cloned(),
            wars_point: user_data
                .get("wars_point")
                .and_then(|p| p.parse().ok())
                .unwrap_or(0.0),
        };

        let token = generate_jwt(&user)?;
        return Ok(token);
    }

    // Create new user
    let user = User {
        id: Uuid::new_v4(),
        wallet_address: wallet_address.clone(),
        display_name: None,
        username: None,
        wars_point: 0.0, // Initialize with 0 wars points
    };

    let user_key = RedisKey::user(KeyPart::Id(user.id));
    let wallets_hash = RedisKey::users_wallets();
    let points_key = RedisKey::users_points();
    let user_id_str = user.id.to_string();

    let user_hash = vec![
        ("id", user.id.to_string()),
        ("wallet_address", user.wallet_address.clone()),
        ("wars_point", user.wars_point.to_string()),
    ];

    let mut pipe = redis::pipe();

    // Store user as Redis hash
    pipe.cmd("HSET").arg(&user_key).arg(&user_hash);

    // Store wallet -> user ID mapping in hash (more memory efficient)
    pipe.cmd("HSET")
        .arg(&wallets_hash)
        .arg(&wallet_address)
        .arg(&user_id_str);

    // Initialize user in wars_points sorted set with 0 points
    pipe.cmd("ZADD").arg(&points_key).arg(0.0).arg(&user_id_str);

    let _: () = pipe
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    let token = generate_jwt(&user)?;
    Ok(token)
}

pub async fn _hydrate_users_points(redis: RedisClient) -> Result<(), AppError> {
    tracing::info!("Starting users_points hydration...");

    // Get all existing users
    let users = _get_all_users(redis.clone()).await?;

    if users.is_empty() {
        tracing::info!("No users found to hydrate");
        return Ok(());
    }

    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let points_key = RedisKey::users_points();

    // Clear existing data in users_points (in case of re-hydration)
    let _: () = conn
        .del(&points_key)
        .await
        .map_err(AppError::RedisCommandError)?;

    // Prepare batch data for ZADD
    let mut zadd_args = Vec::new();
    for user in &users {
        zadd_args.push(user.wars_point.to_string());
        zadd_args.push(user.id.to_string());
    }

    if !zadd_args.is_empty() {
        // Use ZADD to batch insert all users with their wars_point scores
        let _: () = redis::cmd("ZADD")
            .arg(&points_key)
            .arg(&zadd_args)
            .query_async(&mut *conn)
            .await
            .map_err(AppError::RedisCommandError)?;
    }

    tracing::info!(
        "Successfully hydrated users_points sorted set with {} users",
        users.len()
    );

    Ok(())
}

pub async fn create_user_v2(wallet_address: String, postgres: PgPool) -> Result<String, AppError> {
    // Check if user already exists
    let existing_user = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            Option<String>,
            Option<String>,
            f64,
            chrono::NaiveDateTime,
            chrono::NaiveDateTime,
        ),
    >(
        "SELECT id, wallet_address, username, display_name, trust_rating, created_at, updated_at
        FROM users
        WHERE wallet_address = $1",
    )
    .bind(&wallet_address)
    .fetch_optional(&postgres)
    .await
    .map_err(|e| AppError::DatabaseError(format!("Failed to query user: {}", e)))?;

    if let Some((
        id,
        wallet_address,
        username,
        display_name,
        trust_rating,
        created_at,
        updated_at,
    )) = existing_user
    {
        // Get current season ID
        let current_season_id: i32 = get_current_season(postgres.clone()).await?;

        let wars_points = sqlx::query_as::<_, UserWarsPoints>(
            "SELECT id, user_id, season_id, points, rank_badge, created_at, updated_at
                    FROM user_wars_points
                    WHERE user_id = $1 AND season_id = $2",
        )
        .bind(id)
        .bind(current_season_id)
        .fetch_one(&postgres)
        .await
        .map_err(|e| AppError::DatabaseError(format!("Failed to fetch user wars points: {}", e)))?;

        let user = UserV2 {
            id,
            wallet_address,
            username,
            display_name,
            trust_rating,
            created_at,
            updated_at,
            wars_point: wars_points,
        };

        let token = generate_jwt_v2(&user)?;
        return Ok(token);
    }

    // Create new user
    let user_id = Uuid::new_v4();

    // Start a transaction
    let mut tx = postgres
        .begin()
        .await
        .map_err(|e| AppError::DatabaseError(format!("Failed to start transaction: {}", e)))?;

    // Insert user
    sqlx::query(
        "INSERT INTO users (id, wallet_address, trust_rating)
            VALUES ($1, $2, $3)",
    )
    .bind(user_id)
    .bind(&wallet_address)
    .bind(10.0) // Default trust rating
    .execute(&mut *tx)
    .await
    .map_err(|e| AppError::DatabaseError(format!("Failed to create user: {}", e)))?;

    // Insert user_wars_points entry for current season
    let wars_point_id = Uuid::new_v4();

    // Get current season ID
    let current_season_id: i32 = get_current_season(postgres.clone()).await?;

    let wars_points = sqlx::query_as::<_, UserWarsPoints>(
        "INSERT INTO user_wars_points (id, user_id, season_id, points)
        VALUES ($1, $2, $3, $4)
        RETURNING id, user_id, season_id, points, rank_badge, created_at, updated_at",
    )
    .bind(wars_point_id)
    .bind(user_id)
    .bind(current_season_id)
    .bind(0.0) // Initial points
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| AppError::DatabaseError(format!("Failed to create user wars points: {}", e)))?;

    // Commit transaction
    tx.commit()
        .await
        .map_err(|e| AppError::DatabaseError(format!("Failed to commit transaction: {}", e)))?;

    // Fetch the created user with all details
    let user = UserV2 {
        id: user_id,
        wallet_address,
        username: None,
        display_name: None,
        trust_rating: 10.0,
        created_at: chrono::Utc::now().naive_utc(),
        updated_at: chrono::Utc::now().naive_utc(),
        wars_point: wars_points,
    };

    let token = generate_jwt_v2(&user)?;
    tracing::info!("Created new user: {}", user.id);

    Ok(token)
}
