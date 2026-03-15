pub mod routes;
pub mod ws;

use axum::{routing::get, routing::post, Router};
use tower_http::cors::CorsLayer;

use crate::state::AppState;

pub async fn start_server(state: AppState) -> anyhow::Result<u16> {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();

    *state.server_port.write().await = Some(port);

    // Background task: poll Spotify every 3 s and broadcast playback_update over WS.
    let poll_state = state.clone();
    tokio::spawn(async move {
        use std::sync::atomic::Ordering;
        let mut last_track_id: Option<String> = None;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            if !poll_state.party_active.load(Ordering::SeqCst) {
                break;
            }
            let access_token = {
                let spotify = poll_state.spotify.read().await;
                match &*spotify {
                    Some(auth) => auth.access_token.clone(),
                    None => continue,
                }
            };
            if let Ok(pb) = crate::spotify::api::get_playback_state(&access_token).await {
                *poll_state.playback_cache.write().await = pb.clone();
                if let Some(ref playback) = pb {
                    // When the playing track changes, auto-retire it from the party queue.
                    let current_id = playback.track_id.clone();
                    if current_id.is_some() && current_id != last_track_id {
                        if let Some(ref tid) = current_id {
                            // Remove the track from the party queue if it was requested.
                            let retired = {
                                let mut queue = poll_state.queue.write().await;
                                if let Some(pos) = queue.iter().position(|e| &e.track.id == tid) {
                                    Some(queue.remove(pos))
                                } else {
                                    None
                                }
                            };
                            if let Some(entry) = retired {
                                // Clear votes for the retired track.
                                poll_state.votes.write().await.retain(|(vtid, _), _| vtid != tid);
                                // Record in history (newest first, cap 30).
                                {
                                    let mut past = poll_state.past_tracks.write().await;
                                    past.insert(0, entry.track);
                                    past.truncate(30);
                                    let snap = past.clone();
                                    let _ = poll_state.ws_tx.send(
                                        serde_json::json!({"type": "history_update", "tracks": snap})
                                            .to_string(),
                                    );
                                }
                                // Broadcast updated queue.
                                let queue_snap = poll_state.queue.read().await.clone();
                                let _ = poll_state.ws_tx.send(
                                    serde_json::json!({"type": "queue_update", "queue": queue_snap})
                                        .to_string(),
                                );
                            } else {
                                // Track changed but wasn't in the party queue — still record in history.
                                let track = crate::state::Track {
                                    id: tid.clone(),
                                    uri: format!("spotify:track:{}", tid),
                                    title: playback.track_name.clone().unwrap_or_default(),
                                    artist: playback.artist_name.clone().unwrap_or_default(),
                                    album: String::new(),
                                    album_art_url: playback.album_art_url.clone(),
                                    duration_ms: playback.duration_ms.unwrap_or(0),
                                    explicit: false,
                                };
                                let mut past = poll_state.past_tracks.write().await;
                                if past.first().map(|t| &t.id) != Some(tid) {
                                    past.insert(0, track);
                                    past.truncate(30);
                                    let snap = past.clone();
                                    let _ = poll_state.ws_tx.send(
                                        serde_json::json!({"type": "history_update", "tracks": snap})
                                            .to_string(),
                                    );
                                }
                            }
                        }
                        last_track_id = current_id;
                    }
                    let _ = poll_state.ws_tx.send(
                        serde_json::json!({"type": "playback_update", "playback": playback})
                            .to_string(),
                    );
                }
            }
            if let Ok(sq) = crate::spotify::api::get_spotify_queue(&access_token).await {
                *poll_state.spotify_queue_cache.write().await = sq.clone();
                let _ = poll_state.ws_tx.send(
                    serde_json::json!({"type": "spotify_queue_update", "tracks": sq.clone()})
                        .to_string(),
                );

                // Synchronize party queue order with Spotify's actual playback order.
                // Tracks are indexed by their position in Spotify's queue; party tracks
                // not yet visible in Spotify's queue (e.g. just pushed) sort to the end.
                let sq_order: std::collections::HashMap<String, usize> = sq
                    .iter()
                    .enumerate()
                    .map(|(i, t)| (t.id.clone(), i))
                    .collect();
                let order_changed = {
                    let mut queue = poll_state.queue.write().await;
                    let before: Vec<_> = queue.iter().map(|e| e.track.id.clone()).collect();
                    queue.sort_by_key(|e| sq_order.get(&e.track.id).copied().unwrap_or(usize::MAX));
                    let after: Vec<_> = queue.iter().map(|e| e.track.id.clone()).collect();
                    before != after
                };
                if order_changed {
                    let queue_snap = poll_state.queue.read().await.clone();
                    let _ = poll_state.ws_tx.send(
                        serde_json::json!({"type": "queue_update", "queue": queue_snap})
                            .to_string(),
                    );
                }
            }
        }
    });

    let app = build_router(state);

    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .ok();
    });

    Ok(port)
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(serve_guest_ui))
        .route("/api/join", post(routes::join))
        .route("/api/queue", get(routes::get_queue))
        .route("/api/playback", get(routes::get_playback))
        .route("/api/history", get(routes::get_history))
        .route("/api/spotify-queue", get(routes::get_spotify_queue))
        .route("/api/search", get(routes::search))
        .route("/api/request", post(routes::request_track))
        .route("/api/vote", post(routes::vote))
        .route("/ws", get(ws::ws_handler))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn serve_guest_ui() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../../assets/guest.html"))
}
