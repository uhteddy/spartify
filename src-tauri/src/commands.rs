use std::sync::atomic::Ordering;

use serde::Serialize;
use tauri::AppHandle;

use crate::server::routes::broadcast_queue_update;
use crate::spotify::{api, auth};
use crate::state::{AppConfig, AppState, GuestSession, QueueEntry, SpotifyAuth};

// ─── Spotify auth ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct SpotifyStatus {
    pub connected: bool,
    pub client_id: Option<String>,
}

#[tauri::command]
pub async fn get_spotify_status(state: tauri::State<'_, AppState>) -> Result<SpotifyStatus, String> {
    let config = state.config.read().await;
    let connected = state.spotify.read().await.is_some();
    Ok(SpotifyStatus {
        connected,
        client_id: config.spotify_client_id.clone(),
    })
}

/// Saves the client ID and kicks off the PKCE OAuth flow in the browser.
#[tauri::command]
pub async fn connect_spotify(
    client_id: String,
    state: tauri::State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let client_id = client_id.trim().to_string();
    if client_id.is_empty() {
        return Err("Client ID cannot be empty".into());
    }

    let (access_token, refresh_token, expires_at) =
        auth::start_oauth_flow(&client_id, state.inner(), &app)
            .await
            .map_err(|e| e.to_string())?;

    // Persist to config
    {
        let mut config = state.config.write().await;
        config.spotify_client_id = Some(client_id.clone());
        config.refresh_token = Some(refresh_token.clone());
        save_config(&config);
    }

    // Store auth in memory
    *state.spotify.write().await = Some(SpotifyAuth {
        access_token,
        refresh_token,
        expires_at,
        client_id,
    });

    Ok(())
}

/// Dev-mode fallback: manually submit the OAuth code when the deep link
/// can't fire (scheme not registered before first `tauri build`).
#[tauri::command]
pub async fn submit_oauth_code(
    code: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    if let Some(tx) = state.oauth_code_tx.lock().await.take() {
        tx.send(code).map_err(|_| "OAuth flow no longer waiting for a code".to_string())
    } else {
        Err("No OAuth flow is currently in progress".into())
    }
}

// ─── Party lifecycle ──────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct StartPartyResult {
    pub tunnel_url: String,
    pub local_url: String,
}

#[tauri::command]
pub async fn start_party(state: tauri::State<'_, AppState>) -> Result<StartPartyResult, String> {
    if state.party_active.load(Ordering::SeqCst) {
        let url = state.tunnel_url.read().await.clone().unwrap_or_default();
        let port = state.server_port.read().await.unwrap_or(0);
        return Ok(StartPartyResult {
            tunnel_url: url,
            local_url: format!("http://localhost:{}", port),
        });
    }

    // Ensure Spotify is connected
    if state.spotify.read().await.is_none() {
        return Err("Connect to Spotify before starting a party".into());
    }

    // Start the embedded HTTP server
    let port = crate::server::start_server((*state).clone())
        .await
        .map_err(|e| e.to_string())?;

    // Start the bore tunnel
    let (tunnel_url, child) = crate::tunnel::start_bore_tunnel(port)
        .await
        .map_err(|e| e.to_string())?;

    *state.tunnel_url.write().await = Some(tunnel_url.clone());
    *state.tunnel_process.lock().await = Some(child);
    state.party_active.store(true, Ordering::SeqCst);

    Ok(StartPartyResult {
        tunnel_url,
        local_url: format!("http://localhost:{}", port),
    })
}

#[tauri::command]
pub async fn stop_party(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.party_active.store(false, Ordering::SeqCst);

    // Kill the bore tunnel subprocess
    if let Some(mut child) = state.tunnel_process.lock().await.take() {
        child.kill().await.ok();
    }

    *state.tunnel_url.write().await = None;

    // Clear party state
    state.queue.write().await.clear();
    state.guests.write().await.clear();
    state.votes.write().await.clear();

    Ok(())
}

// ─── Queue ────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_queue(state: tauri::State<'_, AppState>) -> Result<Vec<QueueEntry>, String> {
    Ok(state.queue.read().await.clone())
}

/// Pops the top track from the queue and sends it to Spotify.
#[tauri::command]
pub async fn play_next(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let entry = {
        let queue = state.queue.read().await;
        queue.first().cloned()
    };

    let Some(entry) = entry else {
        return Err("Queue is empty".into());
    };

    let access_token = {
        let spotify = state.spotify.read().await;
        match &*spotify {
            Some(auth) => auth.access_token.clone(),
            None => return Err("Spotify not connected".into()),
        }
    };

    api::add_to_spotify_queue(&entry.track.uri, &access_token)
        .await
        .map_err(|e| e.to_string())?;

    // Remove from our queue
    {
        let mut queue = state.queue.write().await;
        if !queue.is_empty() {
            queue.remove(0);
        }
    }

    // Clear votes for this track
    {
        let track_id = entry.track.id.clone();
        let mut votes = state.votes.write().await;
        votes.retain(|(tid, _), _| tid != &track_id);
    }

    broadcast_queue_update(&*state).await;
    Ok(())
}

/// Host removes a specific track from the queue.
#[tauri::command]
pub async fn remove_from_queue(
    track_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    {
        let mut queue = state.queue.write().await;
        queue.retain(|e| e.track.id != track_id);
    }
    {
        let mut votes = state.votes.write().await;
        votes.retain(|(tid, _), _| tid != &track_id);
    }
    broadcast_queue_update(&*state).await;
    Ok(())
}

// ─── Guests ───────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_guests(state: tauri::State<'_, AppState>) -> Result<Vec<GuestSession>, String> {
    let guests = state.guests.read().await;
    Ok(guests.values().cloned().collect())
}

// ─── Playback ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_playback(
    state: tauri::State<'_, AppState>,
) -> Result<Option<api::PlaybackState>, String> {
    let access_token = {
        let spotify = state.spotify.read().await;
        match &*spotify {
            Some(auth) => {
                // Refresh token if close to expiry
                let now = auth::current_time_secs();
                if auth.expires_at <= now + 120 {
                    drop(spotify);
                    refresh_token_if_needed(&state).await?;
                    let spotify = state.spotify.read().await;
                    spotify.as_ref().map(|a| a.access_token.clone()).unwrap_or_default()
                } else {
                    auth.access_token.clone()
                }
            }
            None => return Err("Spotify not connected".into()),
        }
    };

    api::get_playback_state(&access_token)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_tunnel_url(state: tauri::State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(state.tunnel_url.read().await.clone())
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

async fn refresh_token_if_needed(state: &AppState) -> Result<(), String> {
    let (client_id, refresh_token) = {
        let spotify = state.spotify.read().await;
        match &*spotify {
            Some(a) => (a.client_id.clone(), a.refresh_token.clone()),
            None => return Err("Spotify not connected".into()),
        }
    };

    let (access_token, expires_at) = auth::refresh_access_token(&client_id, &refresh_token)
        .await
        .map_err(|e| e.to_string())?;

    let mut spotify = state.spotify.write().await;
    if let Some(auth) = spotify.as_mut() {
        auth.access_token = access_token;
        auth.expires_at = expires_at;
    }

    Ok(())
}

fn save_config(config: &AppConfig) {
    let path = crate::config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    if let Ok(json) = serde_json::to_string_pretty(config) {
        std::fs::write(path, json).ok();
    }
}
