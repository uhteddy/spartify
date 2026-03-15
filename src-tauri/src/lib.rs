mod commands;
mod server;
mod spotify;
mod state;
mod tunnel;

use state::{AppConfig, AppState};
use tauri::Manager;

pub fn config_path() -> std::path::PathBuf {
    let mut path = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    path.push("spartify");
    path.push("config.json");
    path
}

fn load_config() -> AppConfig {
    let path = config_path();
    if path.exists() {
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(config) = serde_json::from_str::<AppConfig>(&data) {
                return config;
            }
        }
    }
    AppConfig::default()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config = load_config();
    let app_state = AppState::new(config);

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::get_spotify_status,
            commands::connect_spotify,
            commands::start_party,
            commands::stop_party,
            commands::get_queue,
            commands::play_next,
            commands::remove_from_queue,
            commands::get_guests,
            commands::get_playback,
            commands::get_tunnel_url,
            commands::submit_oauth_code,
            commands::check_for_updates,
            commands::install_update,
            commands::get_stored_client_id,
            commands::get_access_token,
            commands::set_sdk_device_id,
            commands::get_devices,
            commands::transfer_playback,
            commands::get_party_settings,
            commands::save_party_settings,
            commands::spotify_play,
            commands::spotify_pause,
            commands::spotify_skip_next,
            commands::spotify_skip_previous,
            commands::get_spotify_queue,
            commands::kick_guest,
            commands::ban_guest,
            commands::unban_fingerprint,
            commands::get_banned_fingerprints,
        ])
        .setup(|app| {
            use tauri_plugin_deep_link::DeepLinkExt;

            // Handle deep links — used for the Spotify OAuth callback.
            // When Spotify redirects to spartify://callback?code=..., the OS
            // routes that URI here. We extract the code and fire the oneshot
            // channel that start_oauth_flow() is waiting on.
            let dl_state: AppState = app.state::<AppState>().inner().clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    if url.scheme() == "spartify" {
                        if let Some(code) = url
                            .query_pairs()
                            .find(|(k, _)| k == "code")
                            .map(|(_, v)| v.into_owned())
                        {
                            let state = dl_state.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Some(tx) = state.oauth_code_tx.lock().await.take() {
                                    let _ = tx.send(code);
                                }
                            });
                        }
                    }
                }
            });

            // On startup, silently try to restore the Spotify session
            // using the persisted refresh token.
            let state: AppState = app.state::<AppState>().inner().clone();

            tauri::async_runtime::spawn(async move {
                let (client_id, refresh_token) = {
                    let config = state.config.read().await;
                    (
                        config.spotify_client_id.clone(),
                        config.refresh_token.clone(),
                    )
                };

                if let (Some(client_id), Some(refresh_token)) = (client_id, refresh_token) {
                    match spotify::auth::refresh_access_token(&client_id, &refresh_token).await {
                        Ok((access_token, expires_at)) => {
                            *state.spotify.write().await = Some(state::SpotifyAuth {
                                access_token,
                                refresh_token: refresh_token.clone(),
                                expires_at,
                                client_id: client_id.clone(),
                            });
                        }
                        Err(e) => {
                            eprintln!("Could not restore Spotify session on startup: {}", e);
                        }
                    }
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
