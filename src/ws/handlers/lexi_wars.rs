use axum::{
    extract::{ConnectInfo, Path, Query, State, WebSocketUpgrade, ws::WebSocket},
    http::StatusCode,
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
};

use crate::{
    db,
    games::lexi_wars::{engine::start_auto_start_timer, utils::generate_random_letter},
    models::{
        game::{
            GameData, GameRoom, GameRoomInfo, GameState, Player, PlayerState, RoomPool,
            WsQueryParams,
        },
        lexi_wars::LexiWarsServerMessage,
        word_loader::WORD_LIST,
    },
    state::{AppState, RedisClient},
    ws::handlers::utils::{remove_connection, store_connection_and_send_queued_messages},
};
use crate::{errors::AppError, games::lexi_wars};
use crate::{
    games::lexi_wars::rules::RuleContext,
    state::{ConnectionInfoMap, SharedRooms},
};
use uuid::Uuid;

async fn setup_player_and_room(
    player: &Player,
    room_info: GameRoomInfo,
    players: Vec<Player>,
    pool: Option<RoomPool>,
    rooms: &SharedRooms,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    let mut locked_rooms = rooms.lock().await;

    // Check if this room is already active in memory
    let room = locked_rooms.entry(room_info.id).or_insert_with(|| {
        tracing::info!("Initializing new in-memory GameRoom for {}", room_info.id);
        let word_list = WORD_LIST.clone();

        GameRoom {
            info: room_info.clone(),
            players: players.clone(),
            data: GameData::LexiWar { word_list },
            eliminated_players: vec![],
            current_turn_id: room_info.creator_id,
            used_words: HashMap::new(),
            used_words_global: HashSet::new(),
            rule_context: RuleContext {
                min_word_length: 4,
                random_letter: generate_random_letter(),
            },
            rule_index: 0,
            pool,
            connected_players: room_info.connected_players.clone(),
            connected_players_count: room_info.connected_players.len(),
            game_started: false,
        }
    });

    let already_exists = room.players.iter().any(|p| p.id == player.id);

    if !already_exists {
        tracing::info!(
            "Adding player {} ({}) to room {}",
            player.wallet_address,
            player.id,
            room.info.id
        );
        room.players.push(player.clone());
    } else {
        tracing::info!(
            "Player {} already exists in room {}, skipping re-add",
            player.wallet_address,
            room.info.id
        );
    }

    // Track connected player - add to connected_players if not already there
    let already_connected = room.connected_players.iter().any(|p| p.id == player.id);
    let was_empty = room.connected_players.is_empty();

    if !already_connected {
        room.connected_players.push(player.clone());
    }

    tracing::info!(
        "Player {} connected to room {}. Connected: {}/{}",
        player.wallet_address,
        room.info.id,
        room.connected_players.len(),
        room.players.len()
    );

    // Start auto-start timer when first player connects
    if was_empty && !room.game_started {
        tracing::info!(
            "First player connected, starting auto-start timer for room {}",
            room.info.id
        );
        start_auto_start_timer(
            room.info.id,
            rooms.clone(),
            connections.clone(),
            redis.clone(),
        );
    }
}

pub async fn lexi_wars_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQueryParams>,
    Path(room_id): Path<Uuid>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    tracing::info!("New Lexi-Wars WebSocket connection from {}", addr);

    let player_id = query.user_id;

    let redis = state.redis.clone();
    let connections = state.connections.clone();
    let rooms = state.rooms.clone();

    let room = db::room::get_room_info(room_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    let is_game_started = {
        let rooms_guard = rooms.lock().await;
        if let Some(game_room) = rooms_guard.get(&room_id) {
            game_room.game_started
        } else {
            false // If not in memory, game hasn't started yet
        }
    };

    if room.state != GameState::InProgress {
        tracing::error!("Room {} is not in progress", room_id);
        return Err(AppError::BadRequest("Game not in progress".into()).to_response());
    }
    if is_game_started {
        let is_reconnecting = room.connected_players.iter().any(|p| p.id == player_id);

        if !is_reconnecting {
            tracing::info!(
                "Player {} trying to join after game started - not an initial player",
                player_id
            );

            // Send AlreadyStarted message and close connection
            return Ok(ws.on_upgrade(move |mut socket| async move {
                let already_started_msg = LexiWarsServerMessage::AlreadyStarted;
                let serialized = serde_json::to_string(&already_started_msg).unwrap();
                let _ = socket
                    .send(axum::extract::ws::Message::Text(serialized.into()))
                    .await;
                let _ = socket.close().await;
            }));
        }

        tracing::info!("Player {} reconnecting to started game", player_id);
    }

    let pool = if let Some(ref addr) = room.contract_address {
        if !addr.is_empty() {
            Some(
                db::room::get_room_pool(room_id, redis.clone())
                    .await
                    .map_err(|e| e.to_response())?,
            )
        } else {
            None
        }
    } else {
        None
    };

    let players = db::room::get_room_players(room_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    // TODO: avoid cloning all players
    let players_clone = players.clone();

    let matched_player = players
        .into_iter()
        .find(|p| p.id == player_id && p.state == PlayerState::Ready)
        .ok_or_else(|| {
            AppError::Unauthorized("Player not found or not ready in room".into()).to_response()
        })?;

    tracing::info!(
        "Player {} allowed to join room {}",
        matched_player.wallet_address,
        room_id
    );

    Ok(ws.on_upgrade(move |socket| {
        let room_info = room.clone();
        handle_lexi_wars_socket(
            socket,
            room_id,
            matched_player,
            players_clone,
            pool,
            rooms,
            connections,
            room_info,
            redis,
        )
    }))
}

async fn handle_lexi_wars_socket(
    socket: WebSocket,
    room_id: Uuid,
    player: Player,
    players: Vec<Player>,
    pool: Option<RoomPool>,
    rooms: SharedRooms,
    connections: ConnectionInfoMap,
    room_info: GameRoomInfo,
    redis: RedisClient,
) {
    let (sender, receiver) = socket.split();

    store_connection_and_send_queued_messages(player.id, sender, &connections, &redis).await;

    setup_player_and_room(
        &player,
        room_info,
        players,
        pool,
        &rooms,
        &connections,
        &redis,
    )
    .await;

    lexi_wars::engine::handle_incoming_messages(
        &player,
        room_id,
        receiver,
        rooms.clone(),
        &connections,
        redis,
    )
    .await;

    // Remove player from connected_players when they disconnect, but only if game hasn't started
    {
        let mut rooms_guard = rooms.lock().await;
        if let Some(room) = rooms_guard.get_mut(&room_id) {
            if let Some(pos) = room
                .connected_players
                .iter()
                .position(|p| p.id == player.id)
            {
                // Only remove from connected_players if game hasn't started
                if !room.game_started {
                    room.connected_players.remove(pos);
                    tracing::info!(
                        "Player {} disconnected from room {} (pre-game). Connected: {}/{}",
                        player.wallet_address,
                        room_id,
                        room.connected_players.len(),
                        room.players.len()
                    );
                } else {
                    tracing::info!(
                        "Player {} disconnected from room {} (during game). Keeping in connected_players for game continuity.",
                        player.wallet_address,
                        room_id
                    );
                }

                // If the disconnected player was the current turn, assign to next connected player
                if room.current_turn_id == player.id
                    && !room.connected_players.is_empty()
                    && !room.game_started
                {
                    room.current_turn_id = room.connected_players[0].id;
                    tracing::info!("Reassigned current turn to player {}", room.current_turn_id);
                }
            }
        }
    }

    remove_connection(player.id, &connections).await;
}
