use axum::extract::ws::Message;
use chrono::Utc;
use futures::StreamExt;
use redis::AsyncCommands;
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

use crate::{
    db::{
        game::state::{
            add_eliminated_player, clear_lobby_game_state, get_current_turn,
            get_eliminated_players, get_game_started, set_current_turn, set_game_started,
        },
        leaderboard::patch::update_user_stats,
        lobby::{
            get::{
                get_connected_players_ids, get_current_players_ids, get_lobby_info,
                get_lobby_players,
            },
            patch::update_lobby_state,
            put::{create_current_players, remove_current_player},
        },
    },
    errors::AppError,
    games::stacks_sweepers::utils::{broadcast_to_lobby, broadcast_to_player, create_rng},
    models::{
        game::{LobbyInfo, LobbyState, Player},
        redis::{KeyPart, RedisKey},
        stacks_sweeper::{
            CellState, EliminationReason, MaskedCell, PlayerStanding, StacksSweeperCell,
            StacksSweeperClientMessage, StacksSweeperServerMessage,
        },
    },
    state::{ConnectionInfoMap, RedisClient},
};
use teloxide::Bot;

fn get_prize(
    lobby_info: &LobbyInfo,
    connected_players_count: usize,
    position: usize,
) -> Option<f64> {
    if lobby_info.contract_address.is_none() {
        return None;
    }

    let entry_amount = lobby_info.entry_amount.unwrap_or(0.0);
    let current_amount = lobby_info.current_amount.unwrap_or(0.0);

    // Calculate total pool based on lobby type
    let total_pool = if entry_amount == 0.0 {
        // Sponsored lobby - use current_amount as the pre-funded pool
        current_amount
    } else {
        // Regular paid lobby - calculate from entry amount * connected players
        entry_amount * connected_players_count as f64
    };

    // No prizes if there's no pool
    if total_pool <= 0.0 {
        return None;
    }

    let prize = match position {
        1 => {
            if connected_players_count == 2 {
                (total_pool * 70.0) / 100.0
            } else {
                (total_pool * 50.0) / 100.0
            }
        }
        2 => (total_pool * 30.0) / 100.0,
        3 => (total_pool * 20.0) / 100.0,
        _ => 0.0,
    };

    Some(prize)
}

fn calculate_wars_point(
    lobby_info: &LobbyInfo,
    connected_players_count: usize,
    rank: usize,
    prize: Option<f64>,
    player_id: Uuid,
) -> f64 {
    let base_point = (connected_players_count - rank + 1) * 2;
    let mut total_point = base_point as f64;

    // Add pool bonus if there's a pool (prize and entry amount exist)
    if let (Some(prize_amount), Some(entry_amount)) = (prize, lobby_info.entry_amount) {
        let pool_bonus = if entry_amount != 0.0 {
            (prize_amount / connected_players_count as f64) + (entry_amount / 5.0)
        } else {
            0.0
        };
        total_point += pool_bonus;
    }

    // Add sponsor bonus if this is a sponsored lobby and the player is the sponsor (creator)
    if let (Some(entry_amount), Some(current_amount)) =
        (lobby_info.entry_amount, lobby_info.current_amount)
    {
        if entry_amount == 0.0 && current_amount > 0.0 && player_id == lobby_info.creator.id {
            let sponsor_bonus = 2.5 * connected_players_count as f64;
            total_point += sponsor_bonus;
        }
    }

    // Cap at 50 points maximum
    total_point.min(50.0)
}

async fn send_rank_prize_and_wars_point(
    player_id: Uuid,
    lobby_id: Uuid,
    lobby_info: &LobbyInfo,
    connected_players_count: usize,
    rank: usize,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    let prize = get_prize(lobby_info, connected_players_count, rank);
    let wars_point =
        calculate_wars_point(lobby_info, connected_players_count, rank, prize, player_id);

    // Send rank message
    let rank_msg = StacksSweeperServerMessage::Rank {
        rank: rank.to_string(),
    };
    broadcast_to_player(player_id, lobby_id, &rank_msg, connections, redis).await;

    // Send prize if applicable
    if let Some(amount) = prize {
        let prize_msg = StacksSweeperServerMessage::Prize { amount };
        broadcast_to_player(player_id, lobby_id, &prize_msg, connections, redis).await;
    }

    // Send wars point message
    let wars_point_msg = StacksSweeperServerMessage::WarsPoint { wars_point };
    broadcast_to_player(player_id, lobby_id, &wars_point_msg, connections, redis).await;

    // Update user stats
    match update_user_stats(player_id, lobby_id, rank, prize, wars_point, redis.clone()).await {
        Ok(()) => {
            tracing::info!(
                "Player {} earned {} wars points (rank: {}, prize: {:?})",
                player_id,
                wars_point,
                rank,
                prize
            );
        }
        Err(e) => {
            tracing::error!(
                "Failed to update user stats for player {}: {}",
                player_id,
                e
            );
        }
    }
}

async fn create_multiplayer_board(
    lobby_id: Uuid,
    size: usize,
    risk: f32,
    redis: RedisClient,
) -> Result<Vec<StacksSweeperCell>, AppError> {
    let total_cells = size * size;
    let mine_count = (total_cells as f32 * risk).round() as usize;

    // Create a vector of all possible positions
    let mut positions: Vec<(usize, usize)> = Vec::new();
    for y in 0..size {
        for x in 0..size {
            positions.push((x, y));
        }
    }

    // Randomly select mine positions
    use rand::seq::SliceRandom;
    let mut rng = create_rng();
    positions.shuffle(&mut rng);
    let mine_positions: std::collections::HashSet<(usize, usize)> =
        positions.into_iter().take(mine_count).collect();

    // Create cells
    let mut cells = Vec::new();
    for y in 0..size {
        for x in 0..size {
            let is_mine = mine_positions.contains(&(x, y));
            let mut adjacent = 0;

            if !is_mine {
                // Calculate adjacent mines
                for dy in -1..=1i32 {
                    for dx in -1..=1i32 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let nx = x as i32 + dx;
                        let ny = y as i32 + dy;
                        if nx >= 0 && ny >= 0 && nx < size as i32 && ny < size as i32 {
                            if mine_positions.contains(&(nx as usize, ny as usize)) {
                                adjacent += 1;
                            }
                        }
                    }
                }
            }

            cells.push(StacksSweeperCell {
                x,
                y,
                is_mine,
                adjacent,
                revealed: false,
                flagged: false,
            });
        }
    }

    // Store board in Redis
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let cells_key = RedisKey::lobby_stacks_sweeper_cells(KeyPart::Id(lobby_id));
    let serialized_cells = serde_json::to_string(&cells)
        .map_err(|e| AppError::Serialization(format!("Failed to serialize cells: {}", e)))?;

    let _: () = conn
        .set(&cells_key, serialized_cells)
        .await
        .map_err(AppError::RedisCommandError)?;

    let mine_count_key = RedisKey::lobby_stacks_sweeper_mine_count(KeyPart::Id(lobby_id));
    let _: () = conn
        .set(&mine_count_key, mine_count)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(cells)
}

async fn get_multiplayer_board(
    lobby_id: Uuid,
    redis: RedisClient,
) -> Result<Option<Vec<StacksSweeperCell>>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let cells_key = RedisKey::lobby_stacks_sweeper_cells(KeyPart::Id(lobby_id));
    let serialized_cells: Option<String> = conn
        .get(&cells_key)
        .await
        .map_err(AppError::RedisCommandError)?;

    match serialized_cells {
        Some(data) => {
            let cells = serde_json::from_str(&data).map_err(|e| {
                AppError::Deserialization(format!("Failed to deserialize cells: {}", e))
            })?;
            Ok(Some(cells))
        }
        None => Ok(None),
    }
}

async fn get_revealed_cells(
    lobby_id: Uuid,
    redis: RedisClient,
) -> Result<std::collections::HashSet<(usize, usize)>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let revealed_key = RedisKey::lobby_stacks_sweeper_revealed_cells(KeyPart::Id(lobby_id));
    let revealed_strs: Vec<String> = conn
        .smembers(&revealed_key)
        .await
        .map_err(AppError::RedisCommandError)?;

    let mut revealed_cells = std::collections::HashSet::new();
    for coord_str in revealed_strs {
        if let Ok((x, y)) = serde_json::from_str::<(usize, usize)>(&coord_str) {
            revealed_cells.insert((x, y));
        }
    }

    Ok(revealed_cells)
}

async fn add_revealed_cell(
    lobby_id: Uuid,
    x: usize,
    y: usize,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let revealed_key = RedisKey::lobby_stacks_sweeper_revealed_cells(KeyPart::Id(lobby_id));
    let coord_str = serde_json::to_string(&(x, y))
        .map_err(|e| AppError::Serialization(format!("Failed to serialize coordinate: {}", e)))?;

    let _: () = conn
        .sadd(&revealed_key, coord_str)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(())
}

async fn shift_mine(
    lobby_id: Uuid,
    hit_x: usize,
    hit_y: usize,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut cells = get_multiplayer_board(lobby_id, redis.clone())
        .await?
        .ok_or_else(|| AppError::NotFound("Board not found".into()))?;

    // Find the cell that was hit
    let cell_index = hit_y * (cells.len() as f64).sqrt() as usize + hit_x;
    if cell_index >= cells.len() {
        return Err(AppError::InvalidInput("Invalid cell position".into()));
    }

    // Only shift if it's actually a mine
    if !cells[cell_index].is_mine {
        return Ok(());
    }

    // Find all non-mine cells
    let safe_positions: Vec<usize> = cells
        .iter()
        .enumerate()
        .filter(|(_, cell)| !cell.is_mine)
        .map(|(index, _)| index)
        .collect();

    if safe_positions.is_empty() {
        return Err(AppError::InvalidInput("No safe positions available".into()));
    }

    // Choose a random safe position
    use rand::Rng;
    let mut rng = create_rng();
    let random_index = rng.random_range(0..safe_positions.len());
    let new_position = safe_positions[random_index];

    // Move the mine
    cells[cell_index].is_mine = false;
    cells[new_position].is_mine = true;

    // Recalculate adjacent counts
    let size = (cells.len() as f64).sqrt() as usize;
    for y in 0..size {
        for x in 0..size {
            let cell_idx = y * size + x;
            if !cells[cell_idx].is_mine {
                let mut adjacent = 0;
                for dy in -1..=1i32 {
                    for dx in -1..=1i32 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let nx = x as i32 + dx;
                        let ny = y as i32 + dy;
                        if nx >= 0 && ny >= 0 && nx < size as i32 && ny < size as i32 {
                            let neighbor_idx = ny as usize * size + nx as usize;
                            if cells[neighbor_idx].is_mine {
                                adjacent += 1;
                            }
                        }
                    }
                }
                cells[cell_idx].adjacent = adjacent;
            }
        }
    }

    // Save updated board
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let cells_key = RedisKey::lobby_stacks_sweeper_cells(KeyPart::Id(lobby_id));
    let serialized_cells = serde_json::to_string(&cells)
        .map_err(|e| AppError::Serialization(format!("Failed to serialize cells: {}", e)))?;

    let _: () = conn
        .set(&cells_key, serialized_cells)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(())
}

pub async fn get_mine_count(lobby_id: Uuid, redis: RedisClient) -> Result<u32, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let mine_count_key = RedisKey::lobby_stacks_sweeper_mine_count(KeyPart::Id(lobby_id));
    let mine_count: Option<u32> = conn
        .get(&mine_count_key)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(mine_count.unwrap_or(0))
}

pub async fn generate_masked_cells(
    lobby_id: Uuid,
    redis: RedisClient,
) -> Result<Vec<MaskedCell>, AppError> {
    let cells = get_multiplayer_board(lobby_id, redis.clone())
        .await?
        .ok_or_else(|| AppError::NotFound("Board not found".into()))?;

    let revealed_cells = get_revealed_cells(lobby_id, redis).await?;

    let masked_cells = cells
        .iter()
        .map(|cell| {
            let is_revealed = revealed_cells.contains(&(cell.x, cell.y));
            let state = if is_revealed {
                if cell.is_mine {
                    Some(CellState::Mine)
                } else if cell.adjacent > 0 {
                    Some(CellState::Adjacent {
                        count: cell.adjacent,
                    })
                } else {
                    Some(CellState::Gem)
                }
            } else if cell.flagged {
                Some(CellState::Flagged)
            } else {
                None
            };

            MaskedCell {
                x: cell.x,
                y: cell.y,
                state,
            }
        })
        .collect();

    Ok(masked_cells)
}

async fn handle_cell_reveal(
    lobby_id: Uuid,
    player: &Player,
    x: usize,
    y: usize,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
    telegram_bot: Bot,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Check if game has started
    let game_started = get_game_started(lobby_id, redis.clone()).await?;
    if !game_started {
        return Err("Game has not started yet".into());
    }

    // Check if it's the player's turn
    let current_turn_id = get_current_turn(lobby_id, redis.clone())
        .await?
        .ok_or("No current turn set")?;

    if player.id != current_turn_id {
        return Err("Not your turn".into());
    }

    // Get current board
    let cells = get_multiplayer_board(lobby_id, redis.clone())
        .await?
        .ok_or("Board not found")?;

    let size = (cells.len() as f64).sqrt() as usize;
    let cell_index = y * size + x;

    if cell_index >= cells.len() {
        return Err("Invalid cell position".into());
    }

    let cell = &cells[cell_index];

    // Check if cell is already revealed
    let revealed_cells = get_revealed_cells(lobby_id, redis.clone()).await?;

    if revealed_cells.contains(&(x, y)) {
        return Err("Cell already revealed".into());
    }

    // Check if this is the first move and if we hit a mine, shift it
    let is_first_move = revealed_cells.is_empty();
    if is_first_move && cell.is_mine {
        shift_mine(lobby_id, x, y, redis.clone()).await?;
        tracing::info!("Mine shifted on first move at ({}, {})", x, y);
    }

    // Reveal the cell
    add_revealed_cell(lobby_id, x, y, redis.clone()).await?;

    // Get updated board after potential mine shift
    let updated_cells = get_multiplayer_board(lobby_id, redis.clone())
        .await?
        .ok_or("Board disappeared")?;

    let updated_cell = &updated_cells[cell_index];

    // Get all players for broadcasting
    let players = get_lobby_players(lobby_id, None, redis.clone()).await?;

    // Broadcast cell revealed to all players
    let cell_revealed_msg = StacksSweeperServerMessage::CellRevealed {
        x,
        y,
        cell_state: if updated_cell.is_mine {
            CellState::Mine
        } else if updated_cell.adjacent > 0 {
            CellState::Adjacent {
                count: updated_cell.adjacent,
            }
        } else {
            CellState::Gem
        },
        revealed_by: player
            .user
            .as_ref()
            .and_then(|u| u.username.clone())
            .unwrap_or_else(|| player.id.to_string()),
    };
    broadcast_to_lobby(&cell_revealed_msg, &players, lobby_id, connections, redis).await;

    // Check for mine hit (after potential mine shift, this should only happen on subsequent moves)
    if updated_cell.is_mine {
        // Player hit a mine - eliminate them
        tracing::info!("Player {} hit a mine at ({}, {})", player.id, x, y);

        crate::db::game::state::add_eliminated_player(lobby_id, player.id, redis.clone()).await?;
        remove_current_player(lobby_id, player.id, redis.clone()).await?;

        // Broadcast elimination
        let eliminated_msg = StacksSweeperServerMessage::Eliminated {
            player: player.clone(),
            reason: EliminationReason::HitMine,
            mine_position: Some((x, y)),
        };

        broadcast_to_lobby(&eliminated_msg, &players, lobby_id, connections, redis).await;

        // Check if game should end
        let remaining_players =
            crate::db::lobby::get::get_current_players_ids(lobby_id, redis.clone()).await?;

        if remaining_players.len() <= 1 {
            // Game over
            end_game(lobby_id, connections, redis.clone(), telegram_bot).await?;
        } else {
            // Continue game with remaining players
            advance_turn(lobby_id, &players, connections, redis.clone(), telegram_bot).await?;
        }
    } else {
        // Safe move - check for win condition (all non-mine cells revealed)
        let total_cells = updated_cells.len();
        let mine_count = get_mine_count(lobby_id, redis.clone()).await? as usize;
        let safe_cells = total_cells - mine_count;

        let updated_revealed = get_revealed_cells(lobby_id, redis.clone()).await?;

        if updated_revealed.len() >= safe_cells {
            // All safe cells revealed - game won!
            tracing::info!("All safe cells revealed - ending game");
            end_game(lobby_id, connections, redis.clone(), telegram_bot).await?;
        } else {
            // Continue game - advance turn
            advance_turn(lobby_id, &players, connections, redis.clone(), telegram_bot).await?;
        }
    }

    Ok(())
}

pub async fn handle_incoming_messages(
    player: &Player,
    lobby_id: Uuid,
    mut receiver: impl StreamExt<Item = Result<Message, axum::Error>> + Unpin,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
    _telegram_bot: Bot,
) {
    let _telegram_bot_clone = _telegram_bot.clone();
    while let Some(msg_result) = receiver.next().await {
        match msg_result {
            Ok(msg) => match msg {
                Message::Text(text) => {
                    let parsed = match serde_json::from_str::<StacksSweeperClientMessage>(&text) {
                        Ok(msg) => msg,
                        Err(e) => {
                            tracing::info!("Invalid message format from {}: {}", player.id, e);
                            continue;
                        }
                    };

                    match parsed {
                        StacksSweeperClientMessage::Ping { ts } => {
                            let now = Utc::now().timestamp_millis() as u64;
                            let pong = now.saturating_sub(ts);
                            let pong_msg = StacksSweeperServerMessage::Pong { ts, pong };
                            broadcast_to_player(
                                player.id,
                                lobby_id,
                                &pong_msg,
                                connections,
                                &redis,
                            )
                            .await;
                        }
                        StacksSweeperClientMessage::CellReveal { x, y } => {
                            if let Err(e) = handle_cell_reveal(
                                lobby_id,
                                player,
                                x,
                                y,
                                connections,
                                &redis,
                                _telegram_bot_clone.clone(),
                            )
                            .await
                            {
                                let error_msg = StacksSweeperServerMessage::Error {
                                    message: e.to_string(),
                                };
                                broadcast_to_player(
                                    player.id,
                                    lobby_id,
                                    &error_msg,
                                    connections,
                                    &redis,
                                )
                                .await;
                            }
                        }
                        StacksSweeperClientMessage::CellFlag { x: _, y: _ } => {
                            // Cell flagging doesn't change turn or reveal anything
                            // Just acknowledge the action
                            let error_msg = StacksSweeperServerMessage::Error {
                                message: "Cell flagging not supported in multiplayer".into(),
                            };
                            broadcast_to_player(
                                player.id,
                                lobby_id,
                                &error_msg,
                                connections,
                                &redis,
                            )
                            .await;
                        }
                        _ => {
                            // Other single-player messages not supported in multiplayer
                            let error_msg = StacksSweeperServerMessage::Error {
                                message: "Command not supported in multiplayer".into(),
                            };
                            broadcast_to_player(
                                player.id,
                                lobby_id,
                                &error_msg,
                                connections,
                                &redis,
                            )
                            .await;
                        }
                    }
                }
                Message::Ping(_data) => {
                    tracing::debug!("WebSocket ping from player {}", player.id);
                }
                Message::Pong(_) => {
                    tracing::debug!("WebSocket pong from player {}", player.id);
                }
                Message::Close(_) => {
                    tracing::debug!("WebSocket close from player {}", player.id);
                    break;
                }
                _ => {}
            },
            Err(e) => {
                tracing::debug!("WebSocket error for player {}: {}", player.id, e);
                break;
            }
        }
    }
}

async fn advance_turn(
    lobby_id: Uuid,
    players: &[Player],
    connections: &ConnectionInfoMap,
    redis: RedisClient,
    telegram_bot: Bot,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let current_players = get_current_players_ids(lobby_id, redis.clone()).await?;
    let current_turn_id = get_current_turn(lobby_id, redis.clone()).await?.unwrap();

    // Find next player
    let current_index = current_players
        .iter()
        .position(|&id| id == current_turn_id)
        .unwrap_or(0);

    let next_index = (current_index + 1) % current_players.len();
    let next_player_id = current_players[next_index];

    set_current_turn(lobby_id, next_player_id, redis.clone()).await?;

    // Send turn message to all players
    if let Some(next_player) = players.iter().find(|p| p.id == next_player_id) {
        let turn_msg = StacksSweeperServerMessage::Turn {
            current_turn: next_player.clone(),
            countdown: 30,
        };
        broadcast_to_lobby(&turn_msg, players, lobby_id, connections, &redis).await;

        // Start turn timer
        start_turn_timer(
            next_player_id,
            lobby_id,
            connections.clone(),
            redis,
            telegram_bot,
        );
    }

    Ok(())
}

fn start_turn_timer(
    player_id: Uuid,
    lobby_id: Uuid,
    connections: ConnectionInfoMap,
    redis: RedisClient,
    telegram_bot: Bot,
) {
    tokio::spawn(async move {
        for i in (0..=30).rev() {
            // Check if the turn is still this player's
            match get_current_turn(lobby_id, redis.clone()).await {
                Ok(Some(current_turn_id)) if current_turn_id == player_id => {
                    // Send countdown to current player
                    let countdown_msg = StacksSweeperServerMessage::Countdown { time_remaining: i };
                    broadcast_to_player(player_id, lobby_id, &countdown_msg, &connections, &redis)
                        .await;

                    // Send turn info to all players
                    if let Ok(players) = get_lobby_players(lobby_id, None, redis.clone()).await {
                        if let Some(current_player) =
                            players.iter().find(|p| p.id == current_turn_id)
                        {
                            let turn_msg = StacksSweeperServerMessage::Turn {
                                current_turn: current_player.clone(),
                                countdown: i as u32,
                            };
                            broadcast_to_lobby(&turn_msg, &players, lobby_id, &connections, &redis)
                                .await;
                        }
                    }
                }
                Ok(Some(_)) => {
                    // Turn has already changed, stop timer
                    tracing::info!("Turn changed, stopping timer for player {}", player_id);
                    return;
                }
                Ok(None) => {
                    tracing::error!("No current turn set for lobby {}", lobby_id);
                    return;
                }
                Err(e) => {
                    tracing::error!("Failed to check current turn: {}", e);
                    return;
                }
            }

            sleep(Duration::from_secs(1)).await;
        }

        // Time ran out - eliminate player
        match get_current_turn(lobby_id, redis.clone()).await {
            Ok(Some(current_turn_id)) if current_turn_id == player_id => {
                tracing::info!("Player {} timed out in lobby {}", player_id, lobby_id);

                // Handle turn timeout - eliminate player and advance turn
                if let Ok(_current_players) = get_current_players_ids(lobby_id, redis.clone()).await
                {
                    // Eliminate the player
                    if let Err(e) = add_eliminated_player(lobby_id, player_id, redis.clone()).await
                    {
                        tracing::error!("Failed to eliminate player: {}", e);
                        return;
                    }

                    // Remove from current players
                    if let Err(e) = remove_current_player(lobby_id, player_id, redis.clone()).await
                    {
                        tracing::error!("Failed to remove timed out player from current: {}", e);
                    }

                    // Get player for elimination broadcast
                    if let Ok(players) = get_lobby_players(lobby_id, None, redis.clone()).await {
                        if let Some(eliminated_player) = players.iter().find(|p| p.id == player_id)
                        {
                            let eliminated_msg = StacksSweeperServerMessage::Eliminated {
                                player: eliminated_player.clone(),
                                reason: EliminationReason::Timeout,
                                mine_position: None,
                            };

                            broadcast_to_lobby(
                                &eliminated_msg,
                                &players,
                                lobby_id,
                                &connections,
                                &redis,
                            )
                            .await;
                        }

                        // Check if game should end
                        let remaining_players =
                            match get_current_players_ids(lobby_id, redis.clone()).await {
                                Ok(players) => players,
                                Err(e) => {
                                    tracing::error!("Failed to get remaining players: {}", e);
                                    return;
                                }
                            };

                        if remaining_players.len() <= 1 {
                            // Game over
                            if let Err(e) =
                                end_game(lobby_id, &connections, redis, telegram_bot).await
                            {
                                tracing::error!("Failed to end game: {}", e);
                            }
                        } else {
                            // Continue with next player
                            if let Err(e) =
                                advance_turn(lobby_id, &players, &connections, redis, telegram_bot)
                                    .await
                            {
                                tracing::error!("Failed to advance turn: {}", e);
                            }
                        }
                    }
                }
            }
            Ok(Some(_)) => {
                // Turn has already changed, nothing to do
                tracing::debug!("Turn has already changed for lobby {}", lobby_id);
            }
            Ok(None) => {
                tracing::error!("No current turn set for lobby {}", lobby_id);
            }
            Err(e) => {
                tracing::error!("Failed to check current turn: {}", e);
            }
        }
    });
}

pub fn start_auto_start_timer(
    lobby_id: Uuid,
    connections: ConnectionInfoMap,
    redis: RedisClient,
    telegram_bot: Bot,
) {
    tokio::spawn(async move {
        for i in (0..=15).rev() {
            // Get current lobby state from Redis
            let connected_player_ids =
                match get_connected_players_ids(lobby_id, redis.clone()).await {
                    Ok(ids) => ids,
                    Err(e) => {
                        tracing::error!("Failed to get connected players: {}", e);
                        return;
                    }
                };

            let lobby_players = match get_lobby_players(lobby_id, None, redis.clone()).await {
                Ok(players) => players,
                Err(e) => {
                    tracing::error!("Failed to get lobby players: {}", e);
                    return;
                }
            };

            let connected_count = connected_player_ids.len();
            let total_players = lobby_players.len();

            tracing::info!(
                "Auto-start timer: {}s, connected: {}/{}",
                i,
                connected_count,
                total_players
            );

            // If all players are connected, start immediately
            if connected_count == total_players {
                tracing::info!("All players connected, starting game early");
                if let Err(e) =
                    start_game(lobby_id, &connections, redis.clone(), telegram_bot.clone()).await
                {
                    tracing::error!("Failed to start game: {}", e);
                }
                return;
            }

            // Send countdown update to connected players
            let start_msg = StacksSweeperServerMessage::Start {
                time: i,
                started: false,
            };
            for player_id in &connected_player_ids {
                broadcast_to_player(*player_id, lobby_id, &start_msg, &connections, &redis).await;
            }

            if i == 0 {
                // Timer expired, check if we have sufficient players
                let required_players = std::cmp::max(2, (total_players + 1) / 2);

                tracing::info!(
                    "Auto-start timer expired: connected {}/{}, required: {}",
                    connected_count,
                    total_players,
                    required_players
                );

                if connected_count >= required_players && connected_count >= 2 {
                    tracing::info!(
                        "Sufficient players connected ({}%), starting game",
                        (connected_count * 100) / total_players
                    );
                    if let Err(e) =
                        start_game(lobby_id, &connections, redis.clone(), telegram_bot.clone())
                            .await
                    {
                        tracing::error!("Failed to start game: {}", e);
                    }
                } else {
                    tracing::info!("Not enough players connected, canceling game");
                    let start_failed_msg = StacksSweeperServerMessage::StartFailed;
                    for player_id in &connected_player_ids {
                        broadcast_to_player(
                            *player_id,
                            lobby_id,
                            &start_failed_msg,
                            &connections,
                            &redis,
                        )
                        .await;
                    }

                    // Reset lobby state
                    if let Err(e) =
                        update_lobby_state(lobby_id, LobbyState::Waiting, redis.clone()).await
                    {
                        tracing::error!("Error updating game state to Waiting: {}", e);
                    }
                }
                return;
            }

            sleep(Duration::from_secs(1)).await;
        }
    });
}

async fn start_game(
    lobby_id: Uuid,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
    telegram_bot: Bot,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Get lobby info to get board configuration
    let lobby_info = get_lobby_info(lobby_id, redis.clone()).await?;

    // Use default values if not set
    let size = lobby_info.board_size.unwrap_or(8);
    let risk = lobby_info.mine_risk.unwrap_or(0.15);

    // Create multiplayer board
    create_multiplayer_board(lobby_id, size, risk, redis.clone()).await?;

    // Set game as started
    set_game_started(lobby_id, true, redis.clone()).await?;

    // Get connected players (only those who are actually connected)
    let connected_player_ids = get_connected_players_ids(lobby_id, redis.clone()).await?;

    // Create current players - initially same as connected players
    create_current_players(lobby_id, connected_player_ids.clone(), redis.clone()).await?;

    // Get all players for broadcasting
    let players = get_lobby_players(lobby_id, None, redis.clone()).await?;

    // Initialize first turn with first connected player
    if let Some(&first_player_id) = connected_player_ids.first() {
        set_current_turn(lobby_id, first_player_id, redis.clone()).await?;

        // Send first turn message to all players
        if let Some(first_player) = players.iter().find(|p| p.id == first_player_id) {
            let turn_msg = StacksSweeperServerMessage::Turn {
                current_turn: first_player.clone(),
                countdown: 30,
            };
            broadcast_to_lobby(&turn_msg, &players, lobby_id, connections, &redis).await;
        }

        // Send game started message to all players
        let game_started_msg = StacksSweeperServerMessage::Start {
            time: 0,
            started: true,
        };
        broadcast_to_lobby(&game_started_msg, &players, lobby_id, connections, &redis).await;

        // Send initial board state to all players
        let masked_cells = generate_masked_cells(lobby_id, redis.clone()).await?;
        let board_msg = StacksSweeperServerMessage::GameBoard {
            cells: masked_cells,
            game_state: crate::models::stacks_sweeper::StacksSweeperGameState::Playing,
            time_remaining: Some(30),
            mines: get_mine_count(lobby_id, redis.clone()).await?,
            board_size: size,
        };
        broadcast_to_lobby(&board_msg, &players, lobby_id, connections, &redis).await;

        // Start turn timer for first player
        start_turn_timer(
            first_player_id,
            lobby_id,
            connections.clone(),
            redis,
            telegram_bot,
        );

        tracing::info!(
            "StacksSweeper game started for lobby {} with {} connected players",
            lobby_id,
            connected_player_ids.len()
        );
    }

    Ok(())
}

async fn end_game(
    lobby_id: Uuid,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
    _telegram_bot: Bot,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Update game state first to prevent race conditions
    update_lobby_state(lobby_id, LobbyState::Finished, redis.clone()).await?;

    // Get all players for final standing and broadcast
    let players = get_lobby_players(lobby_id, None, redis.clone()).await?;
    let lobby_info = get_lobby_info(lobby_id, redis.clone()).await?;
    let connected_players = get_connected_players_ids(lobby_id, redis.clone()).await?;
    let connected_players_count = connected_players.len();

    // Handle remaining player(s) - give them final ranking
    if let Ok(remaining_players) = get_current_players_ids(lobby_id, redis.clone()).await {
        for (index, &remaining_player_id) in remaining_players.iter().enumerate() {
            let final_rank = index + 1;
            send_rank_prize_and_wars_point(
                remaining_player_id,
                lobby_id,
                &lobby_info,
                connected_players_count,
                final_rank,
                connections,
                &redis,
            )
            .await;
        }
    }

    // Get eliminated players for final standing
    let eliminated_players = get_eliminated_players(lobby_id, redis.clone()).await?;

    // Create final standing - reverse order so winner is first
    let mut final_standings = Vec::new();

    // Add remaining players first (winners)
    if let Ok(remaining_player_ids) = get_current_players_ids(lobby_id, redis.clone()).await {
        for (index, &player_id) in remaining_player_ids.iter().enumerate() {
            if let Some(mut player) = players.iter().find(|p| p.id == player_id).cloned() {
                let rank = index + 1;
                player.prize = get_prize(&lobby_info, connected_players_count, rank);
                final_standings.push(PlayerStanding { player, rank });
            }
        }
    }

    // Add eliminated players in reverse order (last eliminated gets better rank)
    for (index, &player_id) in eliminated_players.iter().rev().enumerate() {
        if let Some(mut player) = players.iter().find(|p| p.id == player_id).cloned() {
            let rank = final_standings.len() + index + 1;
            player.prize = get_prize(&lobby_info, connected_players_count, rank);
            final_standings.push(PlayerStanding { player, rank });
        }
    }

    // Send game over messages
    let gameover_msg = StacksSweeperServerMessage::MultiplayerGameOver;
    broadcast_to_lobby(&gameover_msg, &players, lobby_id, connections, &redis).await;

    // Broadcast final standing
    let final_standing_msg = StacksSweeperServerMessage::FinalStanding {
        standing: final_standings,
    };
    broadcast_to_lobby(&final_standing_msg, &players, lobby_id, connections, &redis).await;

    // Clean up Redis data
    if let Err(e) = clear_lobby_game_state(lobby_id, redis.clone()).await {
        tracing::error!("Failed to clear lobby game state: {}", e);
    }

    // Clean up StacksSweeper-specific data
    if let Err(e) = clear_stacks_sweeper_game_state(lobby_id, redis.clone()).await {
        tracing::error!("Failed to clear StacksSweeper game state: {}", e);
    }

    tracing::info!("StacksSweeper game ended for lobby {}", lobby_id);
    Ok(())
}

async fn clear_stacks_sweeper_game_state(
    lobby_id: Uuid,
    redis: RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let keys = vec![
        RedisKey::lobby_stacks_sweeper_board(KeyPart::Id(lobby_id)),
        RedisKey::lobby_stacks_sweeper_cells(KeyPart::Id(lobby_id)),
        RedisKey::lobby_stacks_sweeper_revealed_cells(KeyPart::Id(lobby_id)),
        RedisKey::lobby_stacks_sweeper_mine_count(KeyPart::Id(lobby_id)),
    ];

    let _: () = conn.del(&keys).await.map_err(AppError::RedisCommandError)?;

    Ok(())
}
