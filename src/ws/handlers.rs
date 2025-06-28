use axum::{
    extract::{
        ConnectInfo, Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures::SinkExt;
use futures::{StreamExt, stream::SplitSink};
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
    models::game::{GameData, GameRoom, GameRoomInfo, GameState, Player, PlayerState},
    models::word_loader::WORD_LIST,
    state::{AppState, RedisClient},
};
use crate::{
    errors::AppError,
    games::lexi_wars::engine::handle_incoming_messages,
    models::game::{LobbyClientMessage, LobbyServerMessage},
};
use crate::{
    games::lexi_wars::rules::RuleContext,
    state::{PlayerConnections, SharedRooms},
};
use uuid::Uuid;

// TODO: log to centralized logger
async fn store_connection(
    player: &Player,
    sender: SplitSink<WebSocket, Message>,
    connections: &PlayerConnections,
) {
    let mut conns = connections.lock().await;
    conns.insert(player.id, Arc::new(Mutex::new(sender)));
}

async fn remove_connection(player: &Player, connections: &PlayerConnections) {
    let mut conns = connections.lock().await;
    conns.remove(&player.id);
    println!("Player {} disconnected", player.wallet_address);
}

async fn setup_player_and_room(
    player: &Player,
    room_info: GameRoomInfo,
    players: Vec<Player>,
    rooms: &SharedRooms,
    connections: &PlayerConnections,
) {
    let mut locked_rooms = rooms.lock().await;

    // Check if this room is already active in memory
    let room = locked_rooms.entry(room_info.id).or_insert_with(|| {
        println!("Initializing new in-memory GameRoom for {}", room_info.id);
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
        }
    });

    let already_exists = room.players.iter().any(|p| p.id == player.id);

    if !already_exists {
        println!(
            "Adding player {} ({}) to room {}",
            player.wallet_address, player.id, room.info.id
        );
        room.players.push(player.clone());
    } else {
        println!(
            "Player {} already exists in room {}, skipping re-add",
            player.wallet_address, room.info.id
        );
    }

    if let Some(current_player) = room.players.iter().find(|p| p.id == room.current_turn_id) {
        broadcast_to_room(
            "current_turn",
            &current_player.wallet_address,
            &room,
            connections,
        )
        .await;
    }
}

async fn handle_socket(
    socket: WebSocket,
    room_id: Uuid,
    player: Player,
    players: Vec<Player>,
    rooms: SharedRooms,
    connections: PlayerConnections,
    room_info: GameRoomInfo,
    redis: RedisClient,
) {
    let (sender, receiver) = socket.split();

    store_connection(&player, sender, &connections).await;

    setup_player_and_room(&player, room_info, players, &rooms, &connections).await;

    handle_incoming_messages(&player, room_id, receiver, rooms, &connections, redis).await;

    remove_connection(&player, &connections).await;
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
    println!("New WebSocket connection from {}", addr);

    let player_id = query.user_id;

    let redis = state.redis.clone();
    let connections = state.connections.clone();
    let rooms = state.rooms.clone();

    let room = db::room::get_room_info(room_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    if room.state != GameState::InProgress {
        return Err(AppError::BadRequest("Game not in progress".into()).to_response());
    }

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

    println!(
        "Player {} allowed to join room {}",
        matched_player.wallet_address, room_id
    );

    Ok(ws.on_upgrade(move |socket| {
        let room_info = room.clone();
        handle_socket(
            socket,
            room_id,
            matched_player,
            players_clone,
            rooms,
            connections,
            room_info,
            redis,
        )
    }))
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

    let players = db::room::get_room_players(room_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    let matched_player = players
        .iter()
        .find(|p| p.id == player_id)
        .cloned()
        .ok_or_else(|| AppError::Unauthorized("Player not in room".into()).to_response())?;

    tracing::info!(
        "Player {} joined lobby WS {}",
        matched_player.wallet_address,
        room_id
    );

    Ok(ws.on_upgrade(move |socket| {
        handle_lobby_socket(socket, room_id, matched_player, connections, redis)
    }))
}

pub async fn handle_lobby_socket(
    socket: WebSocket,
    room_id: Uuid,
    player: Player,
    connections: PlayerConnections,
    redis: RedisClient,
) {
    let (sender, mut receiver) = socket.split();

    store_connection(&player, sender, &connections).await;

    if let Ok(players) = db::room::get_room_players(room_id, redis.clone()).await {
        let join_msg = LobbyServerMessage::PlayerJoined { players };
        broadcast_to_lobby(room_id, &join_msg, &connections, redis.clone()).await;
    }

    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(parsed) = serde_json::from_str::<LobbyClientMessage>(&text) {
                    match parsed {
                        LobbyClientMessage::UpdatePlayerState { new_state } => {
                            if let Err(e) = db::room::update_player_state(
                                room_id,
                                player.id,
                                new_state,
                                redis.clone(),
                            )
                            .await
                            {
                                tracing::error!("Failed to update state: {}", e);
                            } else if let Ok(players) =
                                db::room::get_room_players(room_id, redis.clone()).await
                            {
                                let msg = LobbyServerMessage::PlayerUpdated { players };
                                broadcast_to_lobby(room_id, &msg, &connections, redis.clone())
                                    .await;
                            }
                        }
                        LobbyClientMessage::LeaveRoom => {
                            if let Err(e) =
                                db::room::leave_room(room_id, player.id, redis.clone()).await
                            {
                                tracing::error!("Failed to leave room: {}", e);
                            } else if let Ok(players) =
                                db::room::get_room_players(room_id, redis.clone()).await
                            {
                                let msg = LobbyServerMessage::PlayerLeft { players };
                                broadcast_to_lobby(room_id, &msg, &connections, redis.clone())
                                    .await;
                            }
                            remove_connection(&player, &connections).await;
                            break;
                        }
                        LobbyClientMessage::KickPlayer {
                            player_id,
                            wallet_address,
                            display_name,
                        } => {
                            let room_info =
                                match db::room::get_room_info(room_id, redis.clone()).await {
                                    Ok(info) => info,
                                    Err(e) => {
                                        tracing::error!("Failed to fetch room info: {}", e);
                                        continue;
                                    }
                                };

                            if room_info.creator_id != player.id {
                                tracing::error!(
                                    "Unauthorized kick attempt by {}",
                                    player.wallet_address
                                );
                                continue;
                            }

                            if room_info.state != GameState::Waiting {
                                tracing::error!("Cannot kick players when game is not waiting");
                                continue;
                            }

                            // Remove player
                            if let Err(e) =
                                db::room::leave_room(room_id, player_id, redis.clone()).await
                            {
                                tracing::error!("Failed to kick player: {}", e);
                            } else if let Ok(players) =
                                db::room::get_room_players(room_id, redis.clone()).await
                            {
                                let msg = LobbyServerMessage::PlayerLeft { players };
                                broadcast_to_lobby(room_id, &msg, &connections, redis.clone())
                                    .await;

                                let kicked_msg = LobbyServerMessage::PlayerKicked {
                                    player_id,
                                    wallet_address,
                                    display_name,
                                };
                                broadcast_to_lobby(
                                    room_id,
                                    &kicked_msg,
                                    &connections,
                                    redis.clone(),
                                )
                                .await;

                                let notify_msg: LobbyServerMessage =
                                    LobbyServerMessage::NotifyKicked;

                                let conns = connections.lock().await;
                                if let Some(sender) = conns.get(&player_id) {
                                    let mut sender = sender.lock().await;
                                    let _ = sender
                                        .send(Message::Text(
                                            serde_json::to_string(&notify_msg).unwrap().into(),
                                        ))
                                        .await;
                                    let _ = sender.send(Message::Close(None)).await;
                                }
                            }

                            // Optionally: disconnect the kicked player
                            let conns = connections.lock().await;
                            if let Some(sender) = conns.get(&player_id) {
                                let mut sender = sender.lock().await;
                                let _ = sender.send(Message::Close(None)).await;
                            }
                        }
                        LobbyClientMessage::UpdateGameState { new_state } => {
                            let room_info =
                                match db::room::get_room_info(room_id, redis.clone()).await {
                                    Ok(info) => info,
                                    Err(e) => {
                                        eprintln!("Failed to fetch room info: {}", e);
                                        continue;
                                    }
                                };

                            if room_info.creator_id != player.id {
                                eprintln!(
                                    "Unauthorized game state update attempt by {}",
                                    player.wallet_address
                                );
                                continue;
                            }

                            if let Err(e) = db::room::update_game_state(
                                room_id,
                                new_state.clone(),
                                redis.clone(),
                            )
                            .await
                            {
                                eprintln!("Failed to update game state: {}", e);
                            } else {
                                if new_state == GameState::InProgress {
                                    for i in (1..=30).rev() {
                                        let countdown_msg =
                                            LobbyServerMessage::Countdown { time: i };
                                        broadcast_to_lobby(
                                            room_id,
                                            &countdown_msg,
                                            &connections,
                                            redis.clone(),
                                        )
                                        .await;
                                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

                                        if let Ok(room_info) =
                                            db::room::get_room_info(room_id, redis.clone()).await
                                        {
                                            if room_info.state != GameState::InProgress {
                                                break;
                                            }
                                        }
                                    }
                                    let game_starting =
                                        LobbyServerMessage::Gamestate { state: new_state };
                                    broadcast_to_lobby(
                                        room_id,
                                        &game_starting,
                                        &connections,
                                        redis.clone(),
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    remove_connection(&player, &connections).await;

    if let Ok(players) = db::room::get_room_players(room_id, redis.clone()).await {
        let msg = LobbyServerMessage::PlayerLeft { players };
        broadcast_to_lobby(room_id, &msg, &connections, redis).await;
    }
}

pub async fn broadcast_to_lobby(
    room_id: Uuid,
    msg: &LobbyServerMessage,
    connections: &PlayerConnections,
    redis: RedisClient,
) {
    let serialized = match serde_json::to_string(msg) {
        Ok(json) => json,
        Err(e) => {
            tracing::error!("Failed to serialize message: {}", e);
            return;
        }
    };

    if let Ok(players) = db::room::get_room_players(room_id, redis).await {
        let connection_guard = connections.lock().await;

        for player in players {
            if let Some(sender_arc) = connection_guard.get(&player.id) {
                let mut sender = sender_arc.lock().await;
                let _ = sender.send(Message::Text(serialized.clone().into())).await;
            }
        }
    }
}
