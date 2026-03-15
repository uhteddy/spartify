use std::sync::atomic::Ordering;

use serde::Serialize;
use tauri::AppHandle;

use crate::server::routes::broadcast_queue_update;
use crate::spotify::{api, auth};
use crate::state::{AppConfig, AppState, GuestSession, PartySettings, PlaybackState, QueueEntry, SpotifyAuth, Track};

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
pub async fn start_party(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<StartPartyResult, String> {
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

    // Start the frp tunnel
    let custom_subdomain = state.config.read().await.party_settings.tunnel_subdomain.clone();
    let (tunnel_url, child) = crate::tunnel::start_frp_tunnel(&app, port, custom_subdomain)
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
    if let Some(child) = state.tunnel_process.lock().await.take() {
        child.kill().ok();
    }

    *state.tunnel_url.write().await = None;

    // Clear party state
    state.queue.write().await.clear();
    state.guests.write().await.clear();
    state.votes.write().await.clear();
    *state.playback_cache.write().await = None;
    *state.server_port.write().await = None;

    Ok(())
}

// ─── Queue ────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_queue(state: tauri::State<'_, AppState>) -> Result<Vec<QueueEntry>, String> {
    Ok(state.queue.read().await.clone())
}

/// Pops the top track from the queue and sends it to Spotify.
#[tauri::command]
pub async fn play_next(_state: tauri::State<'_, AppState>) -> Result<(), String> {
    // Songs are now auto-pushed to Spotify when requested and auto-retired from
    // the party queue when they start playing. This command is kept for API
    // compatibility but is a no-op.
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

/// Removes the guest from the session and sends them a kicked notification.
#[tauri::command]
pub async fn kick_guest(
    guest_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let id: uuid::Uuid = guest_id.parse().map_err(|_| "Invalid guest ID".to_string())?;
    state.guests.write().await.remove(&id);
    let _ = state.ws_tx.send(
        serde_json::json!({"type": "kicked", "guest_id": guest_id}).to_string(),
    );
    crate::server::routes::broadcast_guests_update(&*state).await;
    Ok(())
}

/// Bans the guest's device fingerprint and kicks them immediately.
#[tauri::command]
pub async fn ban_guest(
    guest_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let id: uuid::Uuid = guest_id.parse().map_err(|_| "Invalid guest ID".to_string())?;
    // Grab fingerprint before removing the session.
    let fingerprint = state
        .guests
        .read()
        .await
        .get(&id)
        .and_then(|s| s.fingerprint.clone());
    if let Some(fp) = fingerprint {
        state.banned_fingerprints.write().await.insert(fp);
    }
    state.guests.write().await.remove(&id);
    let _ = state.ws_tx.send(
        serde_json::json!({"type": "kicked", "guest_id": guest_id}).to_string(),
    );
    crate::server::routes::broadcast_guests_update(&*state).await;
    Ok(())
}

/// Lifts a ban by fingerprint.
#[tauri::command]
pub async fn unban_fingerprint(
    fingerprint: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.banned_fingerprints.write().await.remove(&fingerprint);
    Ok(())
}

/// Returns all currently banned fingerprints.
#[tauri::command]
pub async fn get_banned_fingerprints(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    Ok(state.banned_fingerprints.read().await.iter().cloned().collect())
}

// ─── Playback ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_playback(
    state: tauri::State<'_, AppState>,
) -> Result<Option<PlaybackState>, String> {
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

// ─── Device / Playback SDK ────────────────────────────────────────────────────

/// Returns the current Spotify access token so the Web Playback SDK can be initialised.
#[tauri::command]
pub async fn get_access_token(state: tauri::State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(state
        .spotify
        .read()
        .await
        .as_ref()
        .map(|a| a.access_token.clone()))
}

/// Called from the frontend when the Web Playback SDK player is ready.
/// Stores the device_id and transfers playback to it.
#[tauri::command]
pub async fn set_sdk_device_id(
    device_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    *state.active_device_id.write().await = Some(device_id.clone());

    let access_token = {
        let spotify = state.spotify.read().await;
        match &*spotify {
            Some(a) => a.access_token.clone(),
            None => return Ok(()), // SDK ready but not authenticated yet — ignore
        }
    };

    // Transfer playback to Spartify (don't force play — respect current state)
    api::transfer_playback(&device_id, false, &access_token)
        .await
        .map_err(|e| e.to_string())
}

/// Lists all available Spotify Connect devices.
#[tauri::command]
pub async fn get_devices(state: tauri::State<'_, AppState>) -> Result<Vec<api::Device>, String> {
    let access_token = {
        let spotify = state.spotify.read().await;
        match &*spotify {
            Some(a) => a.access_token.clone(),
            None => return Err("Spotify not connected".into()),
        }
    };
    api::get_devices(&access_token).await.map_err(|e| e.to_string())
}

/// Transfers Spotify playback to the given device and updates the active device in state.
#[tauri::command]
pub async fn transfer_playback(
    device_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let access_token = {
        let spotify = state.spotify.read().await;
        match &*spotify {
            Some(a) => a.access_token.clone(),
            None => return Err("Spotify not connected".into()),
        }
    };

    api::transfer_playback(&device_id, false, &access_token)
        .await
        .map_err(|e| e.to_string())?;

    *state.active_device_id.write().await = Some(device_id);
    Ok(())
}

// ─── Party settings ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_party_settings(state: tauri::State<'_, AppState>) -> Result<PartySettings, String> {
    Ok(state.config.read().await.party_settings.clone())
}

#[tauri::command]
pub async fn save_party_settings(
    settings: PartySettings,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    config.party_settings = settings;
    save_config(&config);
    Ok(())
}

// ─── Playback controls ────────────────────────────────────────────────────────

#[tauri::command]
pub async fn spotify_play(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let (token, device_id) = get_token_and_device(&state).await?;
    api::set_playback(true, device_id.as_deref(), &token)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn spotify_pause(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let (token, device_id) = get_token_and_device(&state).await?;
    api::set_playback(false, device_id.as_deref(), &token)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn spotify_skip_next(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let (token, device_id) = get_token_and_device(&state).await?;
    api::skip_next(device_id.as_deref(), &token)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn spotify_skip_previous(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let (token, device_id) = get_token_and_device(&state).await?;
    api::skip_previous(device_id.as_deref(), &token)
        .await
        .map_err(|e| e.to_string())
}

/// Returns what Spotify has queued up next (independent of the Spartify party queue).
#[tauri::command]
pub async fn get_spotify_queue(state: tauri::State<'_, AppState>) -> Result<Vec<Track>, String> {
    let token = {
        let spotify = state.spotify.read().await;
        match &*spotify {
            Some(a) => a.access_token.clone(),
            None => return Err("Spotify not connected".into()),
        }
    };
    api::get_spotify_queue(&token).await.map_err(|e| e.to_string())
}

// ─── Updates ──────────────────────────────────────────────────────────────────

/// Returns the new version string if an update is available, `null` if up to date.
#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_updater::UpdaterExt;
    match app.updater().map_err(|e| e.to_string())?.check().await {
        Ok(Some(update)) => Ok(Some(update.version)),
        Ok(None) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Downloads and installs the latest update, then restarts the app.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No update available".to_string())?;

    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|e| e.to_string())?;

    app.restart();
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

async fn get_token_and_device(state: &AppState) -> Result<(String, Option<String>), String> {
    let token = {
        let spotify = state.spotify.read().await;
        match &*spotify {
            Some(a) => a.access_token.clone(),
            None => return Err("Spotify not connected".into()),
        }
    };
    let device_id = state.active_device_id.read().await.clone();
    Ok((token, device_id))
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
