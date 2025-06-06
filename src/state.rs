use axum::extract::ws::{Message, WebSocket};
use futures::stream::SplitSink;
use redis::aio::ConnectionManager;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub rooms: Rooms,
    pub connections: Connections,
    pub redis: RedisClient,
}

use crate::models::GameRoom;

pub type Rooms = Arc<Mutex<HashMap<Uuid, GameRoom>>>;

pub type Sender = Arc<Mutex<SplitSink<WebSocket, Message>>>;

pub type Connections = Arc<Mutex<HashMap<Uuid, Sender>>>;

pub type RedisClient = Arc<ConnectionManager>;
