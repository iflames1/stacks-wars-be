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
        chat::ChatServerMessage,
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

    // Check if room exists and get room state
    let room_info = db::lobby::get_room_info(room_id, redis.clone())
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
) {
    let (sender, receiver) = socket.split();

    store_chat_connection_and_send_queued_messages(player.id, sender, &chat_connections, &redis)
        .await;

    // Check if player is in the room and send permission status
    let is_room_member = match db::lobby::get_room_players(room_id, redis.clone()).await {
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

    // If player is a room member, send chat history
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
    }

    message_handler::handle_incoming_chat_messages(
        receiver,
        room_id,
        &player,
        &chat_connections,
        redis.clone(),
        chat_histories,
    )
    .await;

    remove_chat_connection(player.id, &chat_connections).await;
}
