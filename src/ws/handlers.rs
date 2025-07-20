use axum::{
    extract::{
        ConnectInfo, Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt, stream::SplitSink};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
};
use tokio::sync::Mutex;

use crate::{
    db,
    games::lexi_wars::utils::{broadcast_to_room, generate_random_letter},
    models::{
        game::{
            GameData, GameRoom, GameRoomInfo, GameState, LexiWarsServerMessage, Player,
            PlayerState, RoomPool,
        },
        lobby::{JoinRequest, JoinState},
        word_loader::WORD_LIST,
    },
    state::{AppState, ConnectionInfo, LobbyJoinRequests, RedisClient},
    ws::lobby,
};
use crate::{errors::AppError, games::lexi_wars, models::game::LobbyServerMessage};
use crate::{
    games::lexi_wars::rules::RuleContext,
    state::{ConnectionInfoMap, SharedRooms},
};
use uuid::Uuid;

// Redis message queue functions
pub async fn queue_message_for_player(
    player_id: Uuid,
    message: String,
    redis: &RedisClient,
) -> Result<(), AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("player_queue:{}", player_id);

    // Use Redis list to store messages with 2-minute TTL
    let _: () = redis::cmd("LPUSH")
        .arg(&key)
        .arg(&message)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    // Set TTL to 2 minutes (120 seconds)
    let _: () = redis::cmd("EXPIRE")
        .arg(&key)
        .arg(120)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    tracing::debug!("Queued message for player {} in Redis", player_id);
    Ok(())
}

pub async fn get_queued_messages_for_player(
    player_id: Uuid,
    redis: &RedisClient,
) -> Result<Vec<String>, AppError> {
    let mut conn = redis.get().await.map_err(|e| match e {
        bb8::RunError::User(err) => AppError::RedisCommandError(err),
        bb8::RunError::TimedOut => AppError::RedisPoolError("Redis connection timed out".into()),
    })?;

    let key = format!("player_queue:{}", player_id);

    // Get all messages and delete the key atomically
    let messages: Vec<String> = redis::cmd("LRANGE")
        .arg(&key)
        .arg(0)
        .arg(-1)
        .query_async(&mut *conn)
        .await
        .map_err(AppError::RedisCommandError)?;

    if !messages.is_empty() {
        // Delete the key after retrieving messages
        let _: () = redis::cmd("DEL")
            .arg(&key)
            .query_async(&mut *conn)
            .await
            .map_err(AppError::RedisCommandError)?;

        tracing::info!(
            "Retrieved {} queued messages for player {}",
            messages.len(),
            player_id
        );
    }

    // Reverse the messages since LPUSH adds to the front but we want chronological order
    Ok(messages.into_iter().rev().collect())
}

async fn store_connection(
    player_id: Uuid,
    sender: SplitSink<WebSocket, Message>,
    connections: &ConnectionInfoMap,
) {
    let mut conns = connections.lock().await;
    let conn_info = ConnectionInfo {
        sender: Arc::new(Mutex::new(sender)),
    };
    conns.insert(player_id, Arc::new(conn_info));
    tracing::debug!("Stored connection for player {}", player_id);
}

async fn store_connection_and_send_queued_messages(
    player_id: Uuid,
    sender: SplitSink<WebSocket, Message>,
    connections: &ConnectionInfoMap,
    redis: &RedisClient,
) {
    // Store the connection first
    store_connection(player_id, sender, connections).await;

    // Check for queued messages and send them
    match get_queued_messages_for_player(player_id, redis).await {
        Ok(messages) => {
            if !messages.is_empty() {
                tracing::info!(
                    "Sending {} queued messages to player {}",
                    messages.len(),
                    player_id
                );

                let conns = connections.lock().await;
                if let Some(conn_info) = conns.get(&player_id) {
                    let mut sender_guard = conn_info.sender.lock().await;

                    let mut sent_count = 0;
                    for message in messages {
                        if let Err(e) = sender_guard.send(Message::Text(message.into())).await {
                            tracing::error!(
                                "Failed to send queued message to player {}: {}",
                                player_id,
                                e
                            );
                            break;
                        }
                        sent_count += 1;

                        // Small delay to avoid overwhelming the client
                        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                    }

                    tracing::info!(
                        "Successfully sent {} queued messages to player {}",
                        sent_count,
                        player_id
                    );
                }
            }
        }
        Err(e) => {
            tracing::error!(
                "Failed to retrieve queued messages for player {}: {}",
                player_id,
                e
            );
        }
    }
}

pub async fn remove_connection(player_id: Uuid, connections: &ConnectionInfoMap) {
    let mut conns = connections.lock().await;
    if conns.remove(&player_id).is_some() {
        tracing::debug!("Removed connection for player {}", player_id);
    }
}

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

    if let Some(current_player) = room.players.iter().find(|p| p.id == room.current_turn_id) {
        let next_turn_msg = LexiWarsServerMessage::Turn {
            current_turn: current_player.clone(),
        };
        broadcast_to_room(&next_turn_msg, &room, &connections, redis).await;
    }
}

#[derive(Deserialize)]
pub struct WsQueryParams {
    user_id: Uuid,
}

pub async fn lexi_wars_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQueryParams>,
    Path(room_id): Path<Uuid>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    tracing::info!("New WebSocket connection from {}", addr);

    let player_id = query.user_id;

    let redis = state.redis.clone();
    let connections = state.connections.clone();
    let rooms = state.rooms.clone();

    let room = db::room::get_room_info(room_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    if room.state != GameState::InProgress {
        tracing::error!("Room {} is not in progress", room_id);
        return Err(AppError::BadRequest("Game not in progress".into()).to_response());
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
        rooms,
        &connections,
        redis,
    )
    .await;

    remove_connection(player.id, &connections).await;
}

pub async fn lobby_ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQueryParams>,
    Path(room_id): Path<Uuid>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, String)> {
    tracing::info!("New lobby WS connection from {}", addr);

    let player_id = query.user_id;
    let redis = state.redis.clone();
    let connections = state.connections.clone();
    let join_requests = state.lobby_join_requests.clone();

    let players = db::room::get_room_players(room_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    if let Some(matched_player) = players.iter().find(|p| p.id == player_id).cloned() {
        return Ok(ws.on_upgrade(move |socket| {
            handle_lobby_socket(
                socket,
                room_id,
                matched_player.into(),
                connections,
                redis,
                join_requests,
            )
        }));
    }

    let user = db::user::get_user_by_id(player_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    {
        let mut guard = join_requests.lock().await;
        let room_requests = guard.entry(room_id).or_default();

        let already_requested = room_requests.iter().any(|req| req.user.id == user.id);
        if !already_requested {
            room_requests.push(JoinRequest {
                user: user.clone(),
                state: JoinState::Idle,
            });
        }
    }

    let idle_player = Player {
        id: user.id,
        wallet_address: user.wallet_address.clone(),
        display_name: user.display_name.clone(),
        state: PlayerState::NotReady,
        rank: None,
        used_words: vec![],
        tx_id: None,
        claim: None,
        prize: None,
    };

    Ok(ws.on_upgrade(move |socket| {
        handle_lobby_socket(
            socket,
            room_id,
            idle_player,
            connections,
            redis,
            join_requests,
        )
    }))
}

async fn handle_lobby_socket(
    socket: WebSocket,
    room_id: Uuid,
    player: Player,
    connections: ConnectionInfoMap,
    redis: RedisClient,
    join_requests: LobbyJoinRequests,
) {
    let (sender, receiver) = socket.split();

    store_connection(player.id, sender, &connections).await;

    if let Ok(players) = db::room::get_room_players(room_id, redis.clone()).await {
        let join_msg = LobbyServerMessage::PlayerJoined { players };
        lobby::broadcast_to_lobby(room_id, &join_msg, &connections, redis.clone()).await;
    }

    lobby::handle_incoming_messages(
        receiver,
        room_id,
        &player,
        &connections,
        redis.clone(),
        join_requests,
    )
    .await;

    remove_connection(player.id, &connections).await;

    if let Ok(players) = db::room::get_room_players(room_id, redis.clone()).await {
        let msg = LobbyServerMessage::PlayerLeft { players };
        lobby::broadcast_to_lobby(room_id, &msg, &connections, redis).await;
    }
}
