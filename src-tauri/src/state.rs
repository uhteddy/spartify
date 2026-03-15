use std::collections::{HashMap, HashSet};
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
    pub explicit: bool,
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
    /// Browser fingerprint hash, used for ban enforcement.
    #[serde(default)]
    pub fingerprint: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SpotifyAuth {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub client_id: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct PartySettings {
    /// Password guests must enter to join. None = no password.
    pub join_password: Option<String>,
    /// Max songs a single guest can have in the queue at once. 0 = unlimited.
    pub requests_per_guest: u32,
    /// Max total queue length. 0 = unlimited.
    pub max_queue_size: u32,
    /// Reject tracks marked explicit by Spotify.
    pub block_explicit: bool,
    /// Custom tunnel subdomain prefix (e.g. "myparty" → myparty.spartify.app).
    /// None = random 8-char subdomain.
    #[serde(default)]
    pub tunnel_subdomain: Option<String>,
    /// Allow guests to vote on the currently playing song to trigger auto-skip.
    #[serde(default)]
    pub auto_skip_enabled: bool,
    /// "percentage" = skip when X% of guests downvote; "count" = skip at X downvotes.
    #[serde(default = "default_auto_skip_mode")]
    pub auto_skip_mode: String,
    /// Threshold value for auto-skip (percentage 0–100 or raw count).
    #[serde(default = "default_auto_skip_threshold")]
    pub auto_skip_threshold: f32,
}

fn default_auto_skip_mode() -> String {
    "percentage".to_string()
}

fn default_auto_skip_threshold() -> f32 {
    50.0
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct AppConfig {
    pub spotify_client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub party_settings: PartySettings,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct PlaybackState {
    pub is_playing: bool,
    pub track_id: Option<String>,
    pub track_name: Option<String>,
    pub artist_name: Option<String>,
    pub album_art_url: Option<String>,
    pub progress_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub device_name: Option<String>,
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
    /// Device ID of the currently selected Spotify Connect / SDK player target.
    pub active_device_id: RwLock<Option<String>>,
    pub tunnel_process: Mutex<Option<tauri_plugin_shell::process::CommandChild>>,
    /// Oneshot sender placed here by the OAuth flow; the deep link handler fires it.
    pub oauth_code_tx: Mutex<Option<tokio::sync::oneshot::Sender<String>>>,
    /// Cached playback state, refreshed every 3 s by the party background poller.
    pub playback_cache: RwLock<Option<PlaybackState>>,
    /// Recently played tracks (newest first, capped at 30).
    pub past_tracks: RwLock<Vec<Track>>,
    /// Spotify's upcoming autoplay queue, refreshed every 3 s by the background poller.
    pub spotify_queue_cache: RwLock<Vec<Track>>,
    /// Fingerprints of banned devices; checked on join.
    pub banned_fingerprints: RwLock<HashSet<String>>,
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
            active_device_id: RwLock::new(None),
            tunnel_process: Mutex::new(None),
            oauth_code_tx: Mutex::new(None),
            playback_cache: RwLock::new(None),
            past_tracks: RwLock::new(Vec::new()),
            spotify_queue_cache: RwLock::new(Vec::new()),
            banned_fingerprints: RwLock::new(HashSet::new()),
        }))
    }
}

impl std::ops::Deref for AppState {
    type Target = AppStateInner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
