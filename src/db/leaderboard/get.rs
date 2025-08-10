use crate::{
    db::{lobby::get::get_lobby_info, user::get::get_all_users},
    errors::AppError,
    models::{
        leaderboard::LeaderBoard,
        redis::{KeyPart, RedisKey},
    },
    state::RedisClient,
};
use redis::AsyncCommands;
use std::collections::HashMap;
use uuid::Uuid;

pub async fn get_leaderboard(redis: RedisClient) -> Result<Vec<LeaderBoard>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    // Get all users
    let users = get_all_users(redis.clone()).await?;
    let mut leaderboards = Vec::new();

    for user in users {
        // Calculate stats for each user
        let (total_matches, total_wins) = calculate_match_stats(user.id, &mut conn).await?;
        let win_rate = if total_matches > 0 {
            (total_wins as f64 / total_matches as f64) * 100.0
        } else {
            0.0
        };
        let pnl = calculate_pnl(user.id, redis.clone()).await?;

        leaderboards.push(LeaderBoard {
            user: user.clone(),
            win_rate,
            rank: 0, // Will be set after sorting
            total_match: total_matches,
            total_wins,
            pnl,
        });
    }

    // Sort by wars_point (descending), then by win_rate (descending) as tiebreaker
    leaderboards.sort_by(|a, b| {
        let wars_point_cmp = b
            .user
            .wars_point
            .partial_cmp(&a.user.wars_point)
            .unwrap_or(std::cmp::Ordering::Equal);
        if wars_point_cmp == std::cmp::Ordering::Equal {
            b.win_rate
                .partial_cmp(&a.win_rate)
                .unwrap_or(std::cmp::Ordering::Equal)
        } else {
            wars_point_cmp
        }
    });

    // Assign ranks
    for (index, leaderboard) in leaderboards.iter_mut().enumerate() {
        leaderboard.rank = (index + 1) as u64;
    }

    Ok(leaderboards)
}

async fn calculate_match_stats(
    user_id: Uuid,
    conn: &mut bb8::PooledConnection<'_, bb8_redis::RedisConnectionManager>,
) -> Result<(u64, u64), AppError> {
    // Get all connected player keys for this user
    let connected_pattern =
        RedisKey::lobby_connected_player(KeyPart::Wildcard, KeyPart::Id(user_id));
    let connected_keys: Vec<String> = redis::cmd("KEYS")
        .arg(&connected_pattern)
        .query_async(&mut **conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    let total_matches = connected_keys.len() as u64;

    // Get all player keys for this user to check wins
    let player_pattern = RedisKey::lobby_player(KeyPart::Wildcard, KeyPart::Id(user_id));
    let player_keys: Vec<String> = redis::cmd("KEYS")
        .arg(&player_pattern)
        .query_async(&mut **conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    let mut total_wins = 0u64;

    for key in player_keys {
        let player_data: HashMap<String, String> = conn
            .hgetall(&key)
            .await
            .map_err(AppError::RedisCommandError)?;

        if let Some(rank_str) = player_data.get("rank") {
            if let Ok(rank) = rank_str.parse::<usize>() {
                if rank == 1 {
                    total_wins += 1;
                }
            }
        }
    }

    Ok((total_matches, total_wins))
}

async fn calculate_pnl(user_id: Uuid, redis: RedisClient) -> Result<f64, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    // Get all player keys for this user
    let player_pattern = RedisKey::lobby_player(KeyPart::Wildcard, KeyPart::Id(user_id));
    let player_keys: Vec<String> = redis::cmd("KEYS")
        .arg(&player_pattern)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    let mut total_pnl = 0.0;
    let mut has_any_prize = false;

    for key in player_keys {
        let player_data: HashMap<String, String> = conn
            .hgetall(&key)
            .await
            .map_err(AppError::RedisCommandError)?;

        // Check if this player has a prize
        if let Some(prize_str) = player_data.get("prize") {
            if let Ok(prize) = prize_str.parse::<f64>() {
                has_any_prize = true;

                // Use centralized key parsing
                if let Some(lobby_id) = RedisKey::extract_lobby_id_from_player_key(&key) {
                    // Get lobby info to get entry_amount
                    match get_lobby_info(lobby_id, redis.clone()).await {
                        Ok(lobby_info) => {
                            let entry_amount = lobby_info.entry_amount.unwrap_or(0.0);
                            total_pnl += prize - entry_amount;
                        }
                        Err(e) => {
                            tracing::warn!("Failed to get lobby info for {}: {}", lobby_id, e);
                        }
                    }
                }
            }
        }
    }

    // Return 0 if no prizes were found across all lobbies
    if !has_any_prize {
        Ok(0.0)
    } else {
        Ok(total_pnl)
    }
}
