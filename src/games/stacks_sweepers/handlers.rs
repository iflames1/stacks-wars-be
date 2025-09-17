use chrono::Utc;
use uuid::Uuid;

use crate::{
    models::{
        game::ClaimState,
        stacks_sweeper::{
            CellState, MaskedCell, StacksSweeperGameState, StacksSweeperServerMessage,
            calc_cashout_multiplier,
        },
    },
    state::{ConnectionInfoMap, RedisClient},
};

use super::{
    single::{broadcast_to_player, get_game_from_redis, save_game_to_redis},
    timer::{handle_timer_start, stop_timer},
};

pub async fn handle_multiplier_target(
    player_id: Uuid,
    size: usize,
    risk: f32,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    // Calculate max multiplier for given size and risk
    let max_multiplier = calc_cashout_multiplier(size, risk as f64, size * size);

    let message = StacksSweeperServerMessage::MultiplierTarget {
        max_multiplier,
        size,
        risk,
    };
    broadcast_to_player(player_id, player_id, &message, connections, redis).await;
}

pub async fn handle_cashout(
    player_id: Uuid,
    tx_id: String,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
) {
    match get_game_from_redis(player_id, &redis).await {
        Ok(mut game) => {
            if !matches!(game.game_state, StacksSweeperGameState::Playing) {
                let error_msg = StacksSweeperServerMessage::Error {
                    message: "Cannot cashout when game is not in progress".to_string(),
                };
                broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
                return;
            }

            if game.user_revealed_count == 0 {
                let error_msg = StacksSweeperServerMessage::Error {
                    message: "Cannot cashout without revealing any cells".to_string(),
                };
                broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
                return;
            }

            // Calculate final cashout amount
            let multiplier = calc_cashout_multiplier(
                game.size,
                game.risk as f64,
                game.user_revealed_count as usize,
            );
            let cashout_amount = game.amount * multiplier;

            // Update game state
            game.game_state = StacksSweeperGameState::Won;
            game.claim_state = Some(ClaimState::Claimed { tx_id });

            // Stop any running timer
            stop_timer(player_id, &redis).await;

            // Save updated game
            if let Err(e) = save_game_to_redis(&game, &redis).await {
                tracing::error!(
                    "Failed to save cashout game for player {}: {}",
                    player_id,
                    e
                );
                let error_msg = StacksSweeperServerMessage::Error {
                    message: "Failed to process cashout".to_string(),
                };
                broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
                return;
            }

            tracing::info!(
                "Player {} cashed out for {:.6} STX ({}x multiplier)",
                player_id,
                cashout_amount,
                multiplier
            );

            // Send claim info with cashout details
            let claim_info_msg = StacksSweeperServerMessage::ClaimInfo {
                claim_state: game.claim_state.clone(),
                cashout_amount: Some(cashout_amount),
                current_multiplier: Some(multiplier),
                revealed_count: Some(game.user_revealed_count as usize),
                size: Some(game.size),
                risk: Some(game.risk),
            };
            broadcast_to_player(player_id, player_id, &claim_info_msg, connections, &redis).await;
        }
        Err(_) => {
            let error_msg = StacksSweeperServerMessage::Error {
                message: "No active game found".to_string(),
            };
            broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
        }
    }
}

pub async fn handle_ping(
    player_id: Uuid,
    ts: u64,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    let now = Utc::now().timestamp_millis() as u64;
    let pong = now.saturating_sub(ts);
    let pong_msg = StacksSweeperServerMessage::Pong { ts, pong };
    broadcast_to_player(player_id, player_id, &pong_msg, connections, redis).await;
}

pub async fn handle_cell_reveal(
    player_id: Uuid,
    x: usize,
    y: usize,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
) {
    match get_game_from_redis(player_id, &redis).await {
        Ok(mut game) => {
            if !matches!(game.game_state, StacksSweeperGameState::Playing) {
                let error_msg = StacksSweeperServerMessage::Error {
                    message: "Cannot reveal cells when game is not active".to_string(),
                };
                broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
                return;
            }

            // Check if coordinates are valid
            if x >= game.size || y >= game.size {
                let error_msg = StacksSweeperServerMessage::Error {
                    message: "Invalid cell coordinates".to_string(),
                };
                broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
                return;
            }

            // Find the cell and check properties before modifying
            let cell_index = y * game.size + x;
            if cell_index >= game.cells.len() {
                let error_msg = StacksSweeperServerMessage::Error {
                    message: "Cell index out of bounds".to_string(),
                };
                broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
                return;
            }

            let is_mine = game.cells[cell_index].is_mine;
            let is_already_revealed = game.cells[cell_index].revealed;
            let is_flagged = game.cells[cell_index].flagged;

            // Check if cell is already revealed or flagged
            if is_already_revealed || is_flagged {
                let error_msg = StacksSweeperServerMessage::Error {
                    message: "Cell is already revealed or flagged".to_string(),
                };
                broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
                return;
            }

            // Reveal the cell
            game.cells[cell_index].revealed = true;
            game.user_revealed_count += 1;

            // Check if it's a mine
            if is_mine {
                // Game over - player hit a mine
                game.game_state = StacksSweeperGameState::Lost;

                // Stop any running timer
                stop_timer(player_id, &redis).await;

                // Save game state
                if let Err(e) = save_game_to_redis(&game, &redis).await {
                    tracing::error!(
                        "Failed to save game state after mine hit for player {}: {}",
                        player_id,
                        e
                    );
                }

                // Send game over message with all cells revealed
                let unmasked_cells = game
                    .cells
                    .iter()
                    .map(|cell| MaskedCell {
                        x: cell.x,
                        y: cell.y,
                        state: if cell.is_mine {
                            Some(CellState::Mine)
                        } else {
                            Some(CellState::Adjacent {
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
                broadcast_to_player(player_id, player_id, &game_over_msg, connections, &redis)
                    .await;

                // Send claim info for lost game
                let claim_info_msg = StacksSweeperServerMessage::ClaimInfo {
                    claim_state: game.claim_state.clone(),
                    cashout_amount: game.get_cashout_amount(),
                    current_multiplier: None,
                    revealed_count: None,
                    size: None,
                    risk: None,
                };
                broadcast_to_player(player_id, player_id, &claim_info_msg, connections, &redis)
                    .await;

                tracing::info!("Player {} hit a mine at ({}, {})", player_id, x, y);
                return;
            }

            // If it's the first reveal, start the timer (30 seconds)
            if game.user_revealed_count == 1 {
                handle_timer_start(player_id, 30, connections, redis.clone()).await;
            }

            // Save updated game state
            if let Err(e) = save_game_to_redis(&game, &redis).await {
                tracing::error!(
                    "Failed to save game state after cell reveal for player {}: {}",
                    player_id,
                    e
                );
                let error_msg = StacksSweeperServerMessage::Error {
                    message: "Failed to save game state".to_string(),
                };
                broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
                return;
            }

            // Send cell reveal response - use Countdown message instead of CellRevealed
            let countdown_msg = StacksSweeperServerMessage::Countdown {
                time_remaining: 30, // Reset to 30 seconds after each reveal
            };
            broadcast_to_player(player_id, player_id, &countdown_msg, connections, &redis).await;

            // Send updated claim info with current multiplier
            let current_multiplier = calc_cashout_multiplier(
                game.size,
                game.risk as f64,
                game.user_revealed_count as usize,
            );

            let claim_info_msg = StacksSweeperServerMessage::ClaimInfo {
                claim_state: game.claim_state.clone(),
                cashout_amount: game.get_cashout_amount(),
                current_multiplier: Some(current_multiplier),
                revealed_count: Some(game.user_revealed_count as usize),
                size: Some(game.size),
                risk: Some(game.risk),
            };
            broadcast_to_player(player_id, player_id, &claim_info_msg, connections, &redis).await;

            tracing::info!(
                "Player {} revealed cell at ({}, {}) - {} cells revealed, multiplier: {:.2}x",
                player_id,
                x,
                y,
                game.user_revealed_count,
                current_multiplier
            );
        }
        Err(_) => {
            let error_msg = StacksSweeperServerMessage::Error {
                message: "No active game found".to_string(),
            };
            broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
        }
    }
}

pub async fn handle_cell_flag(
    player_id: Uuid,
    x: usize,
    y: usize,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
) {
    match get_game_from_redis(player_id, &redis).await {
        Ok(mut game) => {
            if !matches!(game.game_state, StacksSweeperGameState::Playing) {
                let error_msg = StacksSweeperServerMessage::Error {
                    message: "Cannot flag cells when game is not active".to_string(),
                };
                broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
                return;
            }

            // Check if coordinates are valid
            if x >= game.size || y >= game.size {
                let error_msg = StacksSweeperServerMessage::Error {
                    message: "Invalid cell coordinates".to_string(),
                };
                broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
                return;
            }

            // Find the cell
            let cell_index = y * game.size + x;
            if cell_index >= game.cells.len() {
                let error_msg = StacksSweeperServerMessage::Error {
                    message: "Cell index out of bounds".to_string(),
                };
                broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
                return;
            }

            let cell = &game.cells[cell_index];

            // Check if cell is already revealed
            if cell.revealed {
                let error_msg = StacksSweeperServerMessage::Error {
                    message: "Cannot flag revealed cell".to_string(),
                };
                broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
                return;
            }

            // Toggle flag status
            game.cells[cell_index].flagged = !game.cells[cell_index].flagged;
            let is_flagged = game.cells[cell_index].flagged;

            // Save updated game state
            if let Err(e) = save_game_to_redis(&game, &redis).await {
                tracing::error!(
                    "Failed to save game state after cell flag for player {}: {}",
                    player_id,
                    e
                );
                let error_msg = StacksSweeperServerMessage::Error {
                    message: "Failed to save game state".to_string(),
                };
                broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
                return;
            }

            // Send cell flag response - use Countdown message instead of CellFlagged
            let countdown_msg = StacksSweeperServerMessage::Countdown {
                time_remaining: 30, // Keep countdown going
            };
            broadcast_to_player(player_id, player_id, &countdown_msg, connections, &redis).await;

            tracing::info!(
                "Player {} {} cell at ({}, {})",
                player_id,
                if is_flagged { "flagged" } else { "unflagged" },
                x,
                y
            );
        }
        Err(_) => {
            let error_msg = StacksSweeperServerMessage::Error {
                message: "No active game found".to_string(),
            };
            broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
        }
    }
}
