use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

use crate::{
    db::stacks_sweeper::countdown::{
        clear_stacks_sweeper_countdown, clear_timer_session, get_timer_session,
        set_stacks_sweeper_countdown, set_timer_session,
    },
    models::stacks_sweeper::{StacksSweeperGameState, StacksSweeperServerMessage},
    state::{ConnectionInfoMap, RedisClient},
};

use super::single::{broadcast_to_player, get_game_from_redis, save_game_to_redis};

pub async fn handle_timer_start(
    player_id: Uuid,
    duration_seconds: u64,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
) {
    let session_id = Uuid::new_v4();

    // Set the timer session and countdown in Redis
    let _ = set_timer_session(player_id, session_id, redis.clone()).await;
    let _ = set_stacks_sweeper_countdown(player_id, duration_seconds, redis.clone()).await;

    // Spawn timer task
    let connections_clone = connections.clone();
    let redis_clone = redis.clone();
    tokio::spawn(async move {
        countdown_timer(
            player_id,
            session_id,
            duration_seconds,
            connections_clone,
            redis_clone,
        )
        .await;
    });
}

async fn countdown_timer(
    player_id: Uuid,
    session_id: Uuid,
    duration_seconds: u64,
    connections: ConnectionInfoMap,
    redis: RedisClient,
) {
    for remaining in (0..=duration_seconds).rev() {
        // Check if timer session is still valid
        match get_timer_session(player_id, redis.clone()).await {
            Ok(Some(current_session)) if current_session == session_id => {
                // Session is valid, continue
            }
            _ => {
                tracing::debug!("Timer session for player {} is no longer valid", player_id);
                return;
            }
        }

        // Update countdown in Redis
        let _ = set_stacks_sweeper_countdown(player_id, remaining, redis.clone()).await;

        if remaining == 0 {
            // Timer expired - trigger game over logic
            handle_timer_expiry(player_id, &connections, &redis).await;
        } else {
            // Broadcast timer update
            let message = StacksSweeperServerMessage::Countdown {
                time_remaining: remaining,
            };
            broadcast_to_player(player_id, player_id, &message, &connections, &redis).await;
        }

        if remaining > 0 {
            sleep(Duration::from_secs(1)).await;
        }
    }

    // Clear timer session and countdown
    let _ = clear_timer_session(player_id, redis.clone()).await;
    let _ = clear_stacks_sweeper_countdown(player_id, redis.clone()).await;
}

async fn handle_timer_expiry(
    player_id: Uuid,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    tracing::info!("Timer expired for player {}", player_id);

    // Get the current game
    match get_game_from_redis(player_id, redis).await {
        Ok(mut game) => {
            if matches!(game.game_state, StacksSweeperGameState::Playing) {
                // Set game as lost due to timer expiry
                game.game_state = StacksSweeperGameState::Lost;

                // Save updated game state
                if let Err(e) = save_game_to_redis(&game, redis).await {
                    tracing::error!("Failed to save game state for player {}: {}", player_id, e);
                }

                // Send game over message with all cells revealed
                let unmasked_cells = game
                    .cells
                    .iter()
                    .map(|cell| crate::models::stacks_sweeper::MaskedCell {
                        x: cell.x,
                        y: cell.y,
                        state: if cell.is_mine {
                            Some(crate::models::stacks_sweeper::CellState::Mine)
                        } else {
                            Some(crate::models::stacks_sweeper::CellState::Adjacent {
                                count: cell.adjacent,
                            })
                        },
                    })
                    .collect();

                let game_over_msg = StacksSweeperServerMessage::GameOver {
                    won: false,
                    cells: unmasked_cells,
                    mines: game.get_mine_count(),
                    board_size: game.size,
                };
                broadcast_to_player(player_id, player_id, &game_over_msg, connections, redis).await;

                // Send claim info for timer expiry
                let claim_info_msg = StacksSweeperServerMessage::ClaimInfo {
                    claim_state: game.claim_state.clone(),
                    cashout_amount: game.get_cashout_amount(),
                    current_multiplier: None,
                    revealed_count: None,
                    size: None,
                    risk: None,
                };
                broadcast_to_player(player_id, player_id, &claim_info_msg, connections, redis)
                    .await;
            }
        }
        Err(e) => {
            tracing::error!(
                "Failed to get game for timer expiry for player {}: {}",
                player_id,
                e
            );
        }
    }
}

pub async fn stop_timer(player_id: Uuid, redis: &RedisClient) {
    let _ = clear_timer_session(player_id, redis.clone()).await;
    let _ = clear_stacks_sweeper_countdown(player_id, redis.clone()).await;
}
