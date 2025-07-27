use axum::{
    extract::{ConnectInfo, Path, Query, State, WebSocketUpgrade, ws::WebSocket},
    http::StatusCode,
    response::IntoResponse,
};
use futures::StreamExt;
use std::net::SocketAddr;

use crate::{
    db,
    models::{
        chat::{ChatServerMessage, VoiceParticipant},
        game::{GameState, Player, WsQueryParams},
    },
    state::{AppState, ChatHistories, RedisClient},
    ws::handlers::chat::{message_handler, utils::*},
};
use axum::extract::ws::{CloseFrame, Message};
use uuid::Uuid;

pub async fn chat_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQueryParams>,
    Path(room_id): Path<Uuid>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    tracing::info!("New chat WebSocket connection from {}", addr);

    let player_id = query.user_id;
    let redis = state.redis.clone();
    let chat_connections = state.chat_connections.clone();
    let chat_histories = state.chat_histories.clone();
    let voice_participants = state.voice_participants.clone();
    let voice_rooms = state.voice_rooms.clone();

    // Check if room exists and get room state
    let room_info = db::room::get_room_info(room_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    // If game is finished, close connection immediately
    if room_info.state == GameState::Finished {
        tracing::info!(
            "Player {} trying to connect to chat while game is finished",
            player_id
        );

        return Ok(ws.on_upgrade(move |mut socket| async move {
            let close_frame = CloseFrame {
                code: axum::extract::ws::close_code::NORMAL,
                reason: "Game finished - chat unavailable".into(),
            };

            let _ = socket.send(Message::Close(Some(close_frame))).await;
        }));
    }

    // Get user info to create player object
    let user = db::user::get_user_by_id(player_id, redis.clone())
        .await
        .map_err(|e| e.to_response())?;

    let player = Player {
        id: user.id,
        wallet_address: user.wallet_address.clone(),
        display_name: user.display_name.clone(),
        state: crate::models::game::PlayerState::NotReady,
        rank: None,
        used_words: vec![],
        tx_id: None,
        claim: None,
        prize: None,
    };

    Ok(ws.on_upgrade(move |socket| {
        handle_chat_socket(
            socket,
            room_id,
            player,
            chat_connections,
            redis,
            chat_histories,
            voice_participants,
            voice_rooms,
        )
    }))
}

async fn handle_chat_socket(
    socket: WebSocket,
    room_id: Uuid,
    player: Player,
    chat_connections: crate::state::ChatConnectionInfoMap,
    redis: RedisClient,
    chat_histories: ChatHistories,
    voice_participants: crate::state::VoiceParticipants,
    voice_rooms: crate::state::VoiceRooms,
) {
    let (sender, receiver) = socket.split();

    store_chat_connection_and_send_queued_messages(player.id, sender, &chat_connections, &redis)
        .await;

    // Check if player is in the room and send permission status
    let is_room_member = match db::room::get_room_players(room_id, redis.clone()).await {
        Ok(players) => players.iter().any(|p| p.id == player.id),
        Err(e) => {
            tracing::error!("Failed to check room membership: {}", e);
            false
        }
    };

    let permit_msg = ChatServerMessage::PermitChat {
        allowed: is_room_member,
    };
    send_chat_message_to_player(player.id, &permit_msg, &chat_connections, &redis).await;

    // Send voice permission (same as chat permission for now)
    let voice_permit_msg = ChatServerMessage::VoicePermit {
        allowed: is_room_member,
    };
    send_chat_message_to_player(player.id, &voice_permit_msg, &chat_connections, &redis).await;

    // If player is a room member, send chat history and voice participants
    if is_room_member {
        let chat_history = {
            let mut histories = chat_histories.lock().await;
            let history = histories
                .entry(room_id)
                .or_insert_with(|| crate::state::ChatHistory::new());
            history.get_messages()
        };

        if !chat_history.is_empty() {
            let history_msg = ChatServerMessage::ChatHistory {
                messages: chat_history,
            };
            send_chat_message_to_player(player.id, &history_msg, &chat_connections, &redis).await;
        }

        // Send current voice participants
        let voice_participants_list = {
            let participants = voice_participants.lock().await;
            if let Some(room_participants) = participants.get(&room_id) {
                room_participants.values().cloned().collect()
            } else {
                Vec::new()
            }
        };

        let participants_msg = ChatServerMessage::VoiceParticipants {
            participants: voice_participants_list,
        };
        send_chat_message_to_player(player.id, &participants_msg, &chat_connections, &redis).await;

        // Initialize player as voice participant with defaults
        {
            let mut participants = voice_participants.lock().await;
            let room_participants = participants
                .entry(room_id)
                .or_insert_with(std::collections::HashMap::new);

            if !room_participants.contains_key(&player.id) {
                let voice_participant = VoiceParticipant {
                    player: player.clone(),
                    mic_enabled: false, // Default mic off
                    is_muted: false,    // Default not muted (can hear others)
                    is_speaking: false,
                };
                room_participants.insert(player.id, voice_participant.clone());

                // Notify all participants about the new participant
                let update_msg = ChatServerMessage::VoiceParticipantUpdate {
                    participant: voice_participant,
                };

                // Broadcast to all room participants
                for participant in room_participants.values() {
                    send_chat_message_to_player(
                        participant.player.id,
                        &update_msg,
                        &chat_connections,
                        &redis,
                    )
                    .await;
                }
            }
        }
    }

    message_handler::handle_incoming_chat_messages(
        receiver,
        room_id,
        &player,
        &chat_connections,
        redis.clone(),
        chat_histories,
        voice_participants,
        voice_rooms,
    )
    .await;

    remove_chat_connection(player.id, &chat_connections).await;
}
