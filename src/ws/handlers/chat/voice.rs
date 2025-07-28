use mediasoup::prelude::*;
use mediasoup::worker::{WorkerLogLevel, WorkerLogTag};
use std::collections::HashMap;
use std::net::IpAddr;
use std::num::{NonZeroU8, NonZeroU32};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::models::{
    chat::{ChatServerMessage, TransportOptions, VoiceParticipant},
    game::Player,
};
use crate::state::{ChatConnectionInfoMap, RedisClient};
use crate::ws::handlers::chat::utils::send_chat_message_to_player;

/// List of codecs that SFU will accept from clients
fn media_codecs() -> Vec<RtpCodecCapability> {
    vec![RtpCodecCapability::Audio {
        mime_type: MimeTypeAudio::Opus,
        preferred_payload_type: None,
        clock_rate: NonZeroU32::new(48000).unwrap(),
        channels: NonZeroU8::new(2).unwrap(),
        parameters: RtpCodecParametersParameters::from([("useinbandfec", 1_u32.into())]),
        rtcp_feedback: vec![RtcpFeedback::TransportCc],
    }]
}

pub struct VoiceConnection {
    pub router: Router,
    pub consumer_transport: WebRtcTransport,
    pub producer_transport: WebRtcTransport,
    pub producers: Vec<Producer>,
    pub consumers: HashMap<ConsumerId, Consumer>,
    pub participant: VoiceParticipant,
    pub client_rtp_capabilities: Option<mediasoup::rtp_parameters::RtpCapabilities>,
}

impl VoiceConnection {
    pub async fn new(
        player: Player,
        worker_manager: &WorkerManager,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let worker = worker_manager
            .create_worker({
                let mut settings = WorkerSettings::default();
                settings.log_level = WorkerLogLevel::Warn;
                settings.log_tags = vec![
                    WorkerLogTag::Info,
                    WorkerLogTag::Ice,
                    WorkerLogTag::Dtls,
                    WorkerLogTag::Rtp,
                    WorkerLogTag::Srtp,
                    WorkerLogTag::Rtcp,
                ];
                settings
            })
            .await?;

        let router = worker
            .create_router(RouterOptions::new(media_codecs()))
            .await?;

        let listen_ip_str = if std::env::var("RAILWAY_PUBLIC_DOMAIN").is_ok() {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        }
        .to_string();

        let addr = std::env::var("RAILWAY_PUBLIC_DOMAIN").ok();

        let listen_ip: IpAddr = listen_ip_str
            .parse()
            .map_err(|e| format!("Invalid mediasoup listen ip: {} {}", listen_ip_str, e))?;

        let transport_options =
            WebRtcTransportOptions::new(WebRtcTransportListenInfos::new(ListenInfo {
                protocol: Protocol::Udp,
                ip: listen_ip,
                announced_address: addr,
                port: None,
                port_range: None,
                flags: None,
                send_buffer_size: None,
                recv_buffer_size: None,
            }));

        let producer_transport = router
            .create_webrtc_transport(transport_options.clone())
            .await?;
        let consumer_transport = router.create_webrtc_transport(transport_options).await?;

        let participant = VoiceParticipant {
            player,
            mic_enabled: false,
            is_muted: true,
            is_speaking: false,
        };

        Ok(Self {
            router,
            consumer_transport,
            producer_transport,
            producers: Vec::new(),
            consumers: HashMap::new(),
            participant,
            client_rtp_capabilities: None,
        })
    }

    pub fn get_transport_options(&self) -> (TransportOptions, TransportOptions) {
        let consumer_options = TransportOptions {
            id: self.consumer_transport.id(),
            dtls_parameters: self.consumer_transport.dtls_parameters(),
            ice_candidates: self.consumer_transport.ice_candidates().clone(),
            ice_parameters: self.consumer_transport.ice_parameters().clone(),
        };

        let producer_options = TransportOptions {
            id: self.producer_transport.id(),
            dtls_parameters: self.producer_transport.dtls_parameters(),
            ice_parameters: self.producer_transport.ice_parameters().clone(),
            ice_candidates: self.producer_transport.ice_candidates().clone(),
        };

        (consumer_options, producer_options)
    }
}

pub type VoiceConnections = Arc<Mutex<HashMap<Uuid, HashMap<Uuid, VoiceConnection>>>>;

pub async fn send_voice_init(
    player_id: Uuid,
    voice_connection: &VoiceConnection,
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    let (consumer_transport_options, producer_transport_options) =
        voice_connection.get_transport_options();

    let init_message = ChatServerMessage::VoiceInit {
        consumer_transport_options,
        producer_transport_options,
        router_rtp_capabilities: voice_connection.router.rtp_capabilities().clone(),
    };

    send_chat_message_to_player(player_id, &init_message, chat_connections, redis).await;
}

pub async fn broadcast_voice_participants(
    room_id: Uuid,
    voice_connections: &VoiceConnections,
    room_players: &[Player],
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    let connections_guard = voice_connections.lock().await;
    if let Some(room_connections) = connections_guard.get(&room_id) {
        let participants: Vec<VoiceParticipant> = room_connections
            .values()
            .map(|conn| conn.participant.clone())
            .collect();

        let message = ChatServerMessage::VoiceParticipants { participants };

        for player in room_players {
            send_chat_message_to_player(player.id, &message, chat_connections, redis).await;
        }
    }
}

pub async fn broadcast_voice_participant_update(
    _room_id: Uuid,
    participant: VoiceParticipant,
    room_players: &[Player],
    chat_connections: &ChatConnectionInfoMap,
    redis: &RedisClient,
) {
    let message = ChatServerMessage::VoiceParticipantUpdate { participant };

    for player in room_players {
        send_chat_message_to_player(player.id, &message, chat_connections, redis).await;
    }
}
