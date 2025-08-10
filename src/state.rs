use axum::extract::ws::{Message, WebSocket};
use bb8::Pool;
use bb8_redis::RedisConnectionManager;
use futures::stream::SplitSink;
use std::{collections::HashMap, sync::Arc};
use teloxide::Bot;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub lexi_wars_lobbies: LexiWarsLobbies,
    pub connections: ConnectionInfoMap,
    pub chat_connections: ChatConnectionInfoMap,
    pub redis: RedisClient,
    pub lobby_join_requests: LobbyJoinRequests,
    pub bot: Bot,
    pub lobby_countdowns: LobbyCountdowns,
}

use crate::models::{game::LexiWars, lobby::JoinRequest};

#[derive(Debug)]
pub struct ConnectionInfo {
    pub sender: Arc<Mutex<SplitSink<WebSocket, Message>>>,
}

#[derive(Debug)]
pub struct ChatConnectionInfo {
    pub sender: Arc<Mutex<SplitSink<WebSocket, Message>>>,
}

#[derive(Debug, Clone)]
pub struct CountdownState {
    pub current_time: u32,
}

impl Default for CountdownState {
    fn default() -> Self {
        Self { current_time: 15 }
    }
}

pub type LexiWarsLobbies = Arc<Mutex<HashMap<Uuid, LexiWars>>>;

pub type ConnectionInfoMap = Arc<Mutex<HashMap<Uuid, Arc<ConnectionInfo>>>>;

// Single chat connection per player, but track which lobby they're chatting in
pub type ChatConnectionInfoMap = Arc<Mutex<HashMap<Uuid, Arc<ChatConnectionInfo>>>>;

pub type RedisClient = Pool<RedisConnectionManager>;

pub type LobbyJoinRequests = Arc<Mutex<HashMap<Uuid, Vec<JoinRequest>>>>;

pub type LobbyCountdowns = Arc<Mutex<HashMap<Uuid, CountdownState>>>;
