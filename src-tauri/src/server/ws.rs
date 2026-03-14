use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};

use crate::state::AppState;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // Send current state snapshots immediately on connect
    let init_msgs = {
        let queue = state.queue.read().await.clone();
        let playback = state.playback_cache.read().await.clone();
        let past = state.past_tracks.read().await.clone();
        let spotify_queue = state.spotify_queue_cache.read().await.clone();
        vec![
            serde_json::json!({"type": "queue_update", "queue": queue}).to_string(),
            serde_json::json!({"type": "playback_update", "playback": playback}).to_string(),
            serde_json::json!({"type": "history_update", "tracks": past}).to_string(),
            serde_json::json!({"type": "spotify_queue_update", "tracks": spotify_queue}).to_string(),
        ]
    };

    for msg in init_msgs {
        if sender.send(Message::Text(msg.into())).await.is_err() {
            return;
        }
    }

    let mut rx = state.ws_tx.subscribe();

    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(text) => {
                        if sender.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}
