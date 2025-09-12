use axum::{
    extract::{ConnectInfo, Query, State, WebSocketUpgrade, ws::WebSocket},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use std::{net::SocketAddr, time::Duration};
use tokio::time::sleep;
use uuid::Uuid;

use crate::{
    db::stacks_sweeper::countdown::{
        clear_stacks_sweeper_countdown, clear_timer_session, get_stacks_sweeper_countdown,
        get_timer_session, set_stacks_sweeper_countdown, set_timer_session,
    },
    errors::AppError,
    models::{
        game::WsQueryParams,
        redis::{KeyPart, RedisKey},
        stacks_sweeper::{
            CellState, GameState, MaskedCell, StacksSweeperClientMessage, StacksSweeperGame,
            StacksSweeperServerMessage, calc_cashout_multiplier, calc_target_multiplier,
        },
    },
    state::{AppState, ConnectionInfoMap, RedisClient},
    ws::handlers::utils::{
        queue_message_for_player, remove_connection, store_connection_and_send_queued_messages,
    },
};

pub async fn stacks_sweepers_single_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQueryParams>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    tracing::debug!("New StacksSweeper WebSocket connection from {}", addr);

    let player_id = query.user_id;
    let redis = state.redis.clone();
    let connections = state.connections.clone();

    tracing::info!("Player {} connected to StacksSweeper", player_id);

    Ok(ws.on_upgrade(move |socket| {
        handle_stacks_sweeper_socket(socket, player_id, connections, redis)
    }))
}

async fn handle_stacks_sweeper_socket(
    socket: WebSocket,
    player_id: Uuid,
    connections: ConnectionInfoMap,
    redis: RedisClient,
) {
    let (sender, receiver) = socket.split();

    // Store connection - using player_id as lobby_id for single player games
    store_connection_and_send_queued_messages(player_id, player_id, sender, &connections, &redis)
        .await;

    // Try to get existing game and send it if exists
    send_existing_game_if_exists(&player_id, &connections, &redis).await;

    // Handle incoming messages
    handle_incoming_messages(player_id, receiver, &connections, redis.clone()).await;

    // Clean up connection on disconnect
    remove_connection(player_id, &connections).await;
    tracing::debug!("Player {} disconnected from StacksSweeper", player_id);
}

async fn send_existing_game_if_exists(
    player_id: &Uuid,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    match get_game_from_redis(*player_id, redis).await {
        Ok(game) => {
            // Check if game has ended - if so, send unmasked cells with GameOver message
            if matches!(game.game_state, GameState::Won | GameState::Lost) {
                let won = matches!(game.game_state, GameState::Won);
                let unmasked_cells = get_unmasked_cells(&game);
                let message = StacksSweeperServerMessage::GameOver {
                    won,
                    cells: unmasked_cells,
                    mines: game.get_mine_count(),
                    board_size: game.size,
                };
                tracing::debug!("Sending game over state to reconnected player: {message:?}");
                broadcast_to_player(*player_id, *player_id, &message, connections, redis).await;
            } else {
                // Send existing game board with masked cells for ongoing games
                let masked_cells = game.get_masked_cells();

                // Get actual countdown time from Redis if game is playing
                let time_remaining = if matches!(game.game_state, GameState::Playing) {
                    get_stacks_sweeper_countdown(*player_id, redis.clone())
                        .await
                        .unwrap_or(None)
                } else {
                    None
                };

                let message = StacksSweeperServerMessage::GameBoard {
                    cells: masked_cells,
                    game_state: game.game_state,
                    time_remaining,
                    mines: game.get_mine_count(),
                    board_size: game.size,
                };
                tracing::debug!("Sending ongoing game state to reconnected player: {message:?}");
                broadcast_to_player(*player_id, *player_id, &message, connections, redis).await;
            }
        }
        Err(_) => {
            // No existing game - send NoBoard message
            let no_board_msg = StacksSweeperServerMessage::NoBoard {
                message: "No existing game found. Create a new board to start playing.".to_string(),
            };
            broadcast_to_player(*player_id, *player_id, &no_board_msg, connections, redis).await;
            tracing::info!("No existing game found for player {}", player_id);
        }
    }
}

async fn handle_incoming_messages(
    player_id: Uuid,
    mut receiver: impl StreamExt<Item = Result<axum::extract::ws::Message, axum::Error>> + Unpin,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
) {
    while let Some(msg_result) = receiver.next().await {
        match msg_result {
            Ok(msg) => match msg {
                axum::extract::ws::Message::Text(text) => {
                    match serde_json::from_str::<StacksSweeperClientMessage>(&text) {
                        Ok(parsed) => match parsed {
                            StacksSweeperClientMessage::CreateBoard { size, risk, blind } => {
                                handle_create_board(
                                    player_id,
                                    size,
                                    risk,
                                    blind,
                                    connections,
                                    redis.clone(),
                                )
                                .await;
                            }
                            StacksSweeperClientMessage::MultiplierTarget { size, risk } => {
                                handle_multiplier_target(
                                    player_id,
                                    size,
                                    risk,
                                    connections,
                                    &redis,
                                )
                                .await;
                            }
                            StacksSweeperClientMessage::Ping { ts } => {
                                handle_ping(player_id, ts, connections, &redis).await;
                            }
                            StacksSweeperClientMessage::CellReveal { x, y } => {
                                tracing::info!(
                                    "Player {} revealed cell at ({}, {})",
                                    player_id,
                                    x,
                                    y
                                );
                                handle_cell_reveal(player_id, x, y, connections, redis.clone())
                                    .await;
                            }
                            StacksSweeperClientMessage::CellFlag { x, y } => {
                                handle_cell_flag(player_id, x, y, connections, redis.clone()).await;
                            }
                        },
                        Err(e) => {
                            tracing::error!("Failed to parse StacksSweeper message: {}", e);
                            let error_msg = StacksSweeperServerMessage::Error {
                                message: "Invalid message format".to_string(),
                            };
                            broadcast_to_player(
                                player_id,
                                player_id,
                                &error_msg,
                                connections,
                                &redis,
                            )
                            .await;
                        }
                    }
                }
                _ => {}
            },
            Err(e) => {
                tracing::debug!("WebSocket error for player {}: {}", player_id, e);
                break;
            }
        }
    }
}

async fn handle_create_board(
    player_id: Uuid,
    size: usize,
    risk: f32,
    blind: bool,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
) {
    // Validate input parameters
    if size < 3 || size > 10 {
        let error_msg = StacksSweeperServerMessage::Error {
            message: "Grid size must be between 3 and 10".to_string(),
        };
        broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
        let no_board_msg = StacksSweeperServerMessage::NoBoard {
            message: "Error creating board.".to_string(),
        };
        broadcast_to_player(player_id, player_id, &no_board_msg, connections, &redis).await;
        return;
    }
    if risk < 0.1 || risk > 0.9 {
        let error_msg = StacksSweeperServerMessage::Error {
            message: "Risk must be between 0.1 and 0.9".to_string(),
        };
        broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
        let no_board_msg = StacksSweeperServerMessage::NoBoard {
            message: "Error creating board.".to_string(),
        };
        broadcast_to_player(player_id, player_id, &no_board_msg, connections, &redis).await;
        return;
    }

    // Check if player can create a new game
    if let Ok(existing_game) = get_game_from_redis(player_id, &redis).await {
        if !existing_game.can_create_new() {
            let error_msg = StacksSweeperServerMessage::Error {
                message: "Cannot create a new game while current game is in progress".to_string(),
            };
            broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
            return;
        }
    }

    match create_stacks_sweeper_single(player_id, size, risk, blind, redis.clone()).await {
        Ok(_) => {
            // Get the created game and send it to the player
            match get_game_from_redis(player_id, &redis).await {
                Ok(game) => {
                    let masked_cells = game.get_masked_cells();
                    let message = StacksSweeperServerMessage::BoardCreated {
                        cells: masked_cells,
                        game_state: game.game_state,
                        mines: game.get_mine_count(),
                        board_size: game.size,
                    };
                    broadcast_to_player(player_id, player_id, &message, connections, &redis).await;
                    tracing::info!(
                        "Created new StacksSweeper game for player {} with size {}x{} and risk {}",
                        player_id,
                        size,
                        size,
                        risk
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to get created game for player {}: {}", player_id, e);
                    let error_msg = StacksSweeperServerMessage::Error {
                        message: "Failed to retrieve created game".to_string(),
                    };
                    broadcast_to_player(player_id, player_id, &error_msg, connections, &redis)
                        .await;
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to create game for player {}: {}", player_id, e);
            let error_msg = StacksSweeperServerMessage::Error {
                message: "Failed to create game".to_string(),
            };
            broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
        }
    }
}

async fn handle_ping(
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

async fn handle_multiplier_target(
    player_id: Uuid,
    size: usize,
    risk: f32,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    // Validate input parameters
    if size < 3 || size > 10 {
        let error_msg = StacksSweeperServerMessage::Error {
            message: "Grid size must be between 3 and 10".to_string(),
        };
        broadcast_to_player(player_id, player_id, &error_msg, connections, redis).await;
        return;
    }
    if risk < 0.1 || risk > 0.9 {
        let error_msg = StacksSweeperServerMessage::Error {
            message: "Risk must be between 0.1 and 0.9".to_string(),
        };
        broadcast_to_player(player_id, player_id, &error_msg, connections, redis).await;
        return;
    }

    let max_multiplier = calc_target_multiplier(size, risk as f64);

    let multiplier_msg = StacksSweeperServerMessage::MultiplierTarget {
        max_multiplier,
        size,
        risk,
    };

    broadcast_to_player(player_id, player_id, &multiplier_msg, connections, redis).await;
    tracing::info!(
        "Sent multiplier target to player {}: {}x for size {}x{} with risk {}",
        player_id,
        max_multiplier,
        size,
        size,
        risk
    );
}

async fn handle_cell_reveal(
    player_id: Uuid,
    x: usize,
    y: usize,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
) {
    // Get current game state
    let game = match get_game_from_redis(player_id, &redis).await {
        Ok(game) => game,
        Err(e) => {
            tracing::error!("Failed to get game for player {}: {}", player_id, e);
            let error_msg = StacksSweeperServerMessage::Error {
                message: "Unable to get game".to_string(),
            };
            broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
            return;
        }
    };

    // Check if game is in a playable state (Waiting or Playing)
    if !matches!(game.game_state, GameState::Playing | GameState::Waiting) {
        return;
    }

    // Validate coordinates
    if x >= game.size || y >= game.size {
        let error_msg = StacksSweeperServerMessage::Error {
            message: "Invalid coordinates".to_string(),
        };
        broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
        return;
    }

    let cell_index = y * game.size + x;
    let cell = &game.cells[cell_index];

    // Check if cell is already revealed
    if cell.revealed {
        return;
    }

    // Clone the game for processing
    let mut updated_game = game.clone();

    // Handle first move mine shifting
    if updated_game.first_move && cell.is_mine {
        match updated_game.shift_mine(x, y) {
            Ok(_) => {
                // Mine shifted successfully
                tracing::info!("Mine shifted for player {}", player_id);
            }
            Err(_) => {
                tracing::error!("Failed to shift mine for player {}", player_id);
                let error_msg = StacksSweeperServerMessage::Error {
                    message: "Failed to shift mine".to_string(),
                };
                broadcast_to_player(player_id, player_id, &error_msg, connections, &redis).await;
                return;
            }
        }
    }

    // Start countdown timer on first move and transition to Playing state
    if updated_game.first_move {
        updated_game.first_move = false;
        updated_game.game_state = GameState::Playing; // Transition from Waiting to Playing
        start_game_timer(player_id, connections.clone(), redis.clone(), 60);
    } else {
        if matches!(updated_game.game_state, GameState::Playing) {
            // Reset timer to 60 seconds by restarting the timer
            start_game_timer(player_id, connections.clone(), redis.clone(), 60);
        }
    }

    // Reveal the cell and adjacent cells if it's safe
    let cell_index = y * updated_game.size + x;
    if !updated_game.cells[cell_index].is_mine {
        updated_game.user_revealed_count += 1;
    }

    reveal_cells(&mut updated_game, x, y);

    // Check game state
    let game_over = check_game_state(&mut updated_game, x, y, player_id, &redis).await;

    // Save updated game to Redis
    if let Err(_) = save_game_to_redis(&updated_game, &redis).await {
        tracing::error!("Failed to save game for player {}", player_id);
        return;
    }

    // Send appropriate response
    if game_over {
        if let Err(e) = clear_stacks_sweeper_countdown(player_id, redis.clone()).await {
            tracing::error!("Failed to clear countdown for player {}: {}", player_id, e);
        }
        if let Err(e) = clear_timer_session(player_id, redis.clone()).await {
            tracing::error!(
                "Failed to clear timer session for player {}: {}",
                player_id,
                e
            );
        }

        let won = matches!(updated_game.game_state, GameState::Won);
        let unmasked_cells = get_unmasked_cells(&updated_game);

        let game_over_msg = StacksSweeperServerMessage::GameOver {
            won,
            cells: unmasked_cells,
            mines: updated_game.get_mine_count(),
            board_size: updated_game.size,
        };
        broadcast_to_player(player_id, player_id, &game_over_msg, connections, &redis).await;
    } else {
        let masked_cells = updated_game.get_masked_cells();

        let time_remaining = if matches!(updated_game.game_state, GameState::Playing) {
            get_stacks_sweeper_countdown(player_id, redis.clone())
                .await
                .unwrap_or(None)
        } else {
            None
        };

        let board_msg = StacksSweeperServerMessage::GameBoard {
            cells: masked_cells,
            game_state: updated_game.game_state,
            time_remaining,
            mines: updated_game.get_mine_count(),
            board_size: game.size,
        };
        broadcast_to_player(player_id, player_id, &board_msg, connections, &redis).await;

        // Send cashout information if player has revealed any safe cells
        if updated_game.user_revealed_count > 0
            && matches!(updated_game.game_state, GameState::Playing)
        {
            let current_multiplier = calc_cashout_multiplier(
                updated_game.size,
                updated_game.risk as f64,
                updated_game.user_revealed_count,
            );

            let cashout_msg = StacksSweeperServerMessage::Cashout {
                current_multiplier,
                revealed_count: updated_game.user_revealed_count,
                size: updated_game.size,
                risk: updated_game.risk,
            };
            broadcast_to_player(player_id, player_id, &cashout_msg, connections, &redis).await;
        }
    }
}

async fn handle_cell_flag(
    player_id: Uuid,
    x: usize,
    y: usize,
    connections: &ConnectionInfoMap,
    redis: RedisClient,
) {
    // Get current game state
    let mut game = match get_game_from_redis(player_id, &redis).await {
        Ok(game) => game,
        Err(e) => {
            tracing::error!("Failed to get game for player {}: {}", player_id, e);
            return;
        }
    };

    // Check if game is still playing
    if !matches!(game.game_state, GameState::Playing | GameState::Waiting) {
        return;
    }

    // Validate coordinates
    if x >= game.size || y >= game.size {
        return;
    }

    let cell_index = y * game.size + x;

    // Toggle flag only if cell is not revealed
    if !game.cells[cell_index].revealed {
        game.cells[cell_index].flagged = !game.cells[cell_index].flagged;

        // Save updated game to Redis
        if let Err(_) = save_game_to_redis(&game, &redis).await {
            tracing::error!("Failed to save game for player {}", player_id);
            return;
        }

        // Send updated board
        let masked_cells = game.get_masked_cells();

        // Get actual countdown time from Redis (don't reset timer for flagging)
        let time_remaining = if matches!(game.game_state, GameState::Playing) {
            get_stacks_sweeper_countdown(player_id, redis.clone())
                .await
                .unwrap_or(None)
        } else {
            None
        };

        let board_msg = StacksSweeperServerMessage::GameBoard {
            cells: masked_cells,
            game_state: game.game_state,
            time_remaining,
            mines: game.get_mine_count(),
            board_size: game.size,
        };
        broadcast_to_player(player_id, player_id, &board_msg, connections, &redis).await;
    }
}

fn reveal_cells(game: &mut StacksSweeperGame, x: usize, y: usize) {
    let cell_index = y * game.size + x;

    // Reveal the clicked cell
    game.cells[cell_index].revealed = true;

    // If it's not a mine and has no adjacent mines, reveal adjacent cells recursively
    if !game.cells[cell_index].is_mine && game.cells[cell_index].adjacent == 0 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }

                let nx = x as isize + dx;
                let ny = y as isize + dy;

                if nx >= 0 && ny >= 0 && (nx as usize) < game.size && (ny as usize) < game.size {
                    let adj_index = (ny as usize) * game.size + (nx as usize);

                    if !game.cells[adj_index].revealed && !game.cells[adj_index].is_mine {
                        reveal_cells(game, nx as usize, ny as usize);
                    }
                }
            }
        }
    }
}

async fn check_game_state(
    game: &mut StacksSweeperGame,
    x: usize,
    y: usize,
    player_id: Uuid,
    redis: &RedisClient,
) -> bool {
    let cell_index = y * game.size + x;

    // Check if player hit a mine
    if game.cells[cell_index].is_mine {
        game.game_state = GameState::Lost;
        for cell in &mut game.cells {
            cell.revealed = true;
            cell.flagged = false; // Unflag all cells when game ends
        }
        if let Err(e) = clear_stacks_sweeper_countdown(player_id, redis.clone()).await {
            tracing::error!(
                "Failed to clear countdown for player {}: {:?}",
                player_id,
                e
            );
        }
        if let Err(e) = clear_timer_session(player_id, redis.clone()).await {
            tracing::error!(
                "Failed to clear timer session for player {}: {:?}",
                player_id,
                e
            );
        }
        return true;
    }

    // Check if all safe cells are revealed
    let all_safe_revealed = game.cells.iter().all(|cell| cell.is_mine || cell.revealed);

    if all_safe_revealed {
        game.game_state = GameState::Won;
        for cell in &mut game.cells {
            cell.revealed = true;
            cell.flagged = false; // Unflag all cells when game ends
        }
        if let Err(e) = clear_stacks_sweeper_countdown(player_id, redis.clone()).await {
            tracing::error!(
                "Failed to clear countdown for player {}: {:?}",
                player_id,
                e
            );
        }
        if let Err(e) = clear_timer_session(player_id, redis.clone()).await {
            tracing::error!(
                "Failed to clear timer session for player {}: {:?}",
                player_id,
                e
            );
        }
        return true;
    }

    false
}

fn get_unmasked_cells(game: &StacksSweeperGame) -> Vec<MaskedCell> {
    game.cells
        .iter()
        .map(|cell| {
            let state = if cell.flagged {
                Some(CellState::Flagged)
            } else if cell.is_mine {
                Some(CellState::Mine)
            } else if game.blind {
                Some(CellState::Gem)
            } else {
                Some(CellState::Adjacent {
                    count: cell.adjacent,
                })
            };

            MaskedCell {
                x: cell.x,
                y: cell.y,
                state,
            }
        })
        .collect()
}

fn start_game_timer(
    player_id: Uuid,
    connections: ConnectionInfoMap,
    redis: RedisClient,
    start_time: u64,
) {
    tokio::spawn(async move {
        // Generate a unique session ID for this timer
        let session_id = Uuid::new_v4();

        // Set the timer session in Redis
        if let Err(e) = set_timer_session(player_id, session_id, redis.clone()).await {
            tracing::error!(
                "Failed to set timer session for player {}: {}",
                player_id,
                e
            );
            return;
        }

        for remaining in (1..=start_time).rev() {
            // Check if this timer session is still active
            match get_timer_session(player_id, redis.clone()).await {
                Ok(Some(current_session)) if current_session == session_id => {
                    // This timer is still active, continue
                }
                _ => {
                    // Timer session has been replaced or cleared, stop this timer
                    tracing::debug!(
                        "Timer session replaced for player {} - stopping timer",
                        player_id
                    );
                    return;
                }
            }

            // Check if game is still in playing state before continuing
            if let Ok(game) = get_game_from_redis(player_id, &redis).await {
                if !matches!(game.game_state, GameState::Playing) {
                    // Game is no longer playing, stop the timer and clear session
                    tracing::debug!(
                        "Timer stopped for player {} - game not in playing state",
                        player_id
                    );
                    let _ = clear_timer_session(player_id, redis.clone()).await;
                    return;
                }
            } else {
                // Game not found, stop the timer and clear session
                tracing::debug!("Timer stopped for player {} - game not found", player_id);
                let _ = clear_timer_session(player_id, redis.clone()).await;
                return;
            }

            if let Err(e) = set_stacks_sweeper_countdown(player_id, remaining, redis.clone()).await
            {
                tracing::error!("Failed to save countdown for player {}: {}", player_id, e);
            }

            sleep(Duration::from_secs(1)).await;

            let countdown_msg = StacksSweeperServerMessage::Countdown {
                time_remaining: remaining,
            };

            broadcast_to_player(player_id, player_id, &countdown_msg, &connections, &redis).await;
        }

        // Check one more time if this timer session is still active before ending the game
        match get_timer_session(player_id, redis.clone()).await {
            Ok(Some(current_session)) if current_session == session_id => {
                // This timer is still active, proceed with time up logic
            }
            _ => {
                // Timer session has been replaced, don't end the game
                tracing::debug!(
                    "Timer session replaced for player {} - not ending game",
                    player_id
                );
                return;
            }
        }

        // Time's up - end the game
        if let Ok(mut game) = get_game_from_redis(player_id, &redis).await {
            if matches!(game.game_state, GameState::Playing) {
                game.game_state = GameState::Lost;
                for cell in &mut game.cells {
                    cell.revealed = true;
                    cell.flagged = false; // Unflag all cells when game ends
                }
                let _ = save_game_to_redis(&game, &redis).await;

                if let Err(e) = clear_stacks_sweeper_countdown(player_id, redis.clone()).await {
                    tracing::error!("Failed to clear countdown for player {}: {}", player_id, e);
                }

                // Clear the timer session
                if let Err(e) = clear_timer_session(player_id, redis.clone()).await {
                    tracing::error!(
                        "Failed to clear timer session for player {}: {}",
                        player_id,
                        e
                    );
                }

                let unmasked_cells = get_unmasked_cells(&game);
                let time_up_msg = StacksSweeperServerMessage::TimeUp {
                    cells: unmasked_cells,
                    mines: game.get_mine_count(),
                    board_size: game.size,
                };

                broadcast_to_player(player_id, player_id, &time_up_msg, &connections, &redis).await;
            }
        }
    });
}

async fn create_stacks_sweeper_single(
    user_id: Uuid,
    size: usize,
    risk: f32,
    blind: bool,
    redis: RedisClient,
) -> Result<Uuid, AppError> {
    use crate::{games::stacks_sweepers::Board, models::stacks_sweeper::StacksSweeperGame};

    // Generate the board
    let board = Board::generate(size, risk);
    let game_cells = board.to_game_cells();

    // Create the game instance
    let mut game = StacksSweeperGame::new(user_id, size, risk, game_cells);
    game.blind = blind;
    let game_id = game.id;

    // Store in Redis
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let game_key = RedisKey::stacks_sweeper(KeyPart::Id(user_id));
    let game_hash = game.to_redis_hash();

    // Store the game data
    let _: () = redis::cmd("HSET")
        .arg(&game_key)
        .arg(
            game_hash
                .iter()
                .flat_map(|(k, v)| [k.as_ref(), v.as_str()])
                .collect::<Vec<&str>>(),
        )
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(game_id)
}

async fn get_game_from_redis(
    player_id: Uuid,
    redis: &RedisClient,
) -> Result<StacksSweeperGame, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let game_key = RedisKey::stacks_sweeper(KeyPart::Id(player_id));

    let game_hash: std::collections::HashMap<String, String> = redis::cmd("HGETALL")
        .arg(&game_key)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    if game_hash.is_empty() {
        return Err(AppError::NotFound("Game not found".into()));
    }

    StacksSweeperGame::from_redis_hash(game_hash)
        .map_err(|e| AppError::Deserialization(format!("Failed to deserialize game: {}", e)))
}

async fn save_game_to_redis(game: &StacksSweeperGame, redis: &RedisClient) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let game_key = RedisKey::stacks_sweeper(KeyPart::Id(game.user_id));
    let game_hash = game.to_redis_hash();

    let _: () = redis::cmd("HSET")
        .arg(&game_key)
        .arg(
            game_hash
                .iter()
                .flat_map(|(k, v)| [k.as_ref(), v.as_str()])
                .collect::<Vec<&str>>(),
        )
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    Ok(())
}

// Helper function to broadcast StacksSweeper messages to a specific player
async fn broadcast_to_player(
    player_id: Uuid,
    lobby_id: Uuid, // Using same as player_id for single player games
    message: &StacksSweeperServerMessage,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    let serialized = match serde_json::to_string(message) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to serialize StacksSweeper message: {}", e);
            return;
        }
    };

    // Try to send directly to connected player
    {
        let connections_guard = connections.lock().await;
        if let Some(connection_info) = connections_guard.get(&player_id) {
            let mut sender = connection_info.sender.lock().await;
            if let Err(e) = sender
                .send(axum::extract::ws::Message::Text(serialized.clone().into()))
                .await
            {
                tracing::error!("Failed to send message to player {}: {}", player_id, e);
                // Connection might be closed, remove it
                drop(sender);
                drop(connections_guard);
                remove_connection(player_id, connections).await;
            } else {
                return; // Successfully sent
            }
        }
    }

    // If direct send failed or player not connected, queue the message if it should be queued
    if message.should_queue() {
        if let Err(e) = queue_message_for_player(player_id, lobby_id, serialized, redis).await {
            tracing::error!("Failed to queue message for player {}: {}", player_id, e);
        }
    }
}
