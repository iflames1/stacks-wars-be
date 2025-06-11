use axum::extract::ws::{Message, WebSocket};
use bb8::Pool;
use bb8_redis::RedisConnectionManager;
use futures::stream::SplitSink;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub rooms: SharedRooms,
    pub connections: PlayerConnections,
    pub redis: RedisClient,
}

use crate::models::game::GameRoom;

pub type SharedRooms = Arc<Mutex<HashMap<Uuid, GameRoom>>>;

pub type Sender = Arc<Mutex<SplitSink<WebSocket, Message>>>;

pub type PlayerConnections = Arc<Mutex<HashMap<Uuid, Sender>>>;

pub type RedisClient = Pool<RedisConnectionManager>;
