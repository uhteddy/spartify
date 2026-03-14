use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::{RwLock, Mutex, broadcast};
use uuid::Uuid;
use serde::{Serialize, Deserialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Track {
    pub id: String,
    pub uri: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_art_url: Option<String>,
    pub duration_ms: u64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct QueueEntry {
    pub track: Track,
    pub votes: i32,
    pub requested_by: Uuid,
    pub requested_at: u64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct GuestSession {
    pub id: Uuid,
    pub name: String,
    pub joined_at: u64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SpotifyAuth {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub client_id: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct AppConfig {
    pub spotify_client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub refresh_token: Option<String>,
}

pub struct AppStateInner {
    pub queue: RwLock<Vec<QueueEntry>>,
    pub guests: RwLock<HashMap<Uuid, GuestSession>>,
    pub spotify: RwLock<Option<SpotifyAuth>>,
    /// (track_id, guest_id) -> vote value (1 = up, -1 = down)
    pub votes: RwLock<HashMap<(String, Uuid), i8>>,
    pub tunnel_url: RwLock<Option<String>>,
    pub ws_tx: broadcast::Sender<String>,
    pub config: RwLock<AppConfig>,
    pub server_port: RwLock<Option<u16>>,
    pub party_active: AtomicBool,
    pub tunnel_process: Mutex<Option<tauri_plugin_shell::process::CommandChild>>,
    /// Oneshot sender placed here by the OAuth flow; the deep link handler fires it.
    pub oauth_code_tx: Mutex<Option<tokio::sync::oneshot::Sender<String>>>,
}

#[derive(Clone)]
pub struct AppState(pub Arc<AppStateInner>);

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        let (ws_tx, _) = broadcast::channel(256);
        AppState(Arc::new(AppStateInner {
            queue: RwLock::new(Vec::new()),
            guests: RwLock::new(HashMap::new()),
            spotify: RwLock::new(None),
            votes: RwLock::new(HashMap::new()),
            tunnel_url: RwLock::new(None),
            ws_tx,
            config: RwLock::new(config),
            server_port: RwLock::new(None),
            party_active: AtomicBool::new(false),
            tunnel_process: Mutex::new(None),
            oauth_code_tx: Mutex::new(None),
        }))
    }
}

impl std::ops::Deref for AppState {
    type Target = AppStateInner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
