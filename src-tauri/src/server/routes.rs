use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::spotify::api;
use crate::state::{AppState, GuestSession, PlaybackState, QueueEntry, Track};

// ─── Helper ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TokenParams {
    pub token: Uuid,
}

async fn require_guest(state: &AppState, token: Uuid) -> Result<(), (StatusCode, String)> {
    if state.guests.read().await.contains_key(&token) {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "Invalid or missing guest token".into()))
    }
}

pub async fn broadcast_queue_update(state: &AppState) {
    let queue = state.queue.read().await.clone();
    let _ = state.ws_tx.send(
        serde_json::json!({
            "type": "queue_update",
            "queue": queue,
        })
        .to_string(),
    );
}

pub async fn broadcast_guests_update(state: &AppState) {
    let guests = state.guests.read().await;
    let list: Vec<_> = guests.values().cloned().collect();
    drop(guests);
    let _ = state.ws_tx.send(
        serde_json::json!({
            "type": "guests_update",
            "guests": list,
        })
        .to_string(),
    );
}

fn current_time() -> u64 {
    crate::spotify::auth::current_time_secs()
}

// ─── GET /api/info ────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct PartyInfo {
    requires_password: bool,
    auto_skip_enabled: bool,
}

pub async fn get_info(State(state): State<AppState>) -> Json<PartyInfo> {
    let settings = state.config.read().await.party_settings.clone();
    let requires_password = settings
        .join_password
        .as_deref()
        .map_or(false, |p| !p.is_empty());
    Json(PartyInfo {
        requires_password,
        auto_skip_enabled: settings.auto_skip_enabled,
    })
}

// ─── GET /api/playback ────────────────────────────────────────────────────────

pub async fn get_playback(
    State(state): State<AppState>,
    Query(params): Query<TokenParams>,
) -> Result<Json<Option<PlaybackState>>, (StatusCode, String)> {
    require_guest(&state, params.token).await?;
    Ok(Json(state.playback_cache.read().await.clone()))
}

// ─── GET /api/history ─────────────────────────────────────────────────────────

pub async fn get_history(
    State(state): State<AppState>,
    Query(params): Query<TokenParams>,
) -> Result<Json<Vec<Track>>, (StatusCode, String)> {
    require_guest(&state, params.token).await?;
    Ok(Json(state.past_tracks.read().await.clone()))
}

// ─── GET /api/spotify-queue ───────────────────────────────────────────────────

pub async fn get_spotify_queue(
    State(state): State<AppState>,
    Query(params): Query<TokenParams>,
) -> Result<Json<Vec<Track>>, (StatusCode, String)> {
    require_guest(&state, params.token).await?;
    Ok(Json(state.spotify_queue_cache.read().await.clone()))
}

// ─── GET /api/queue ───────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct QueueResponse {
    queue: Vec<QueueEntry>,
}

pub async fn get_queue(
    State(state): State<AppState>,
    Query(params): Query<TokenParams>,
) -> Result<Json<QueueResponse>, (StatusCode, String)> {
    require_guest(&state, params.token).await?;
    let queue = state.queue.read().await.clone();
    Ok(Json(QueueResponse { queue }))
}

// ─── POST /api/join ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct JoinBody {
    name: String,
    password: Option<String>,
    fingerprint: Option<String>,
}

#[derive(Serialize)]
pub struct JoinResponse {
    guest_token: Uuid,
    name: String,
}

pub async fn join(
    State(state): State<AppState>,
    Json(body): Json<JoinBody>,
) -> Result<Json<JoinResponse>, (StatusCode, String)> {
    let name = body.name.trim().to_string();
    if name.is_empty() || name.len() > 32 {
        return Err((StatusCode::BAD_REQUEST, "Name must be 1–32 characters".into()));
    }

    // Check ban list
    if let Some(ref fp) = body.fingerprint {
        if state.banned_fingerprints.read().await.contains(fp.as_str()) {
            return Err((StatusCode::FORBIDDEN, "You have been banned from this party".into()));
        }
    }

    // Check password if one is set
    let settings = state.config.read().await.party_settings.clone();
    if let Some(ref required) = settings.join_password {
        if !required.is_empty() {
            match body.password.as_deref() {
                Some(p) if p == required.as_str() => {}
                _ => return Err((StatusCode::FORBIDDEN, "Incorrect party password".into())),
            }
        }
    }

    let guest_id = Uuid::new_v4();
    let session = GuestSession {
        id: guest_id,
        name: name.clone(),
        joined_at: current_time(),
        fingerprint: body.fingerprint.clone(),
    };

    state.guests.write().await.insert(guest_id, session);
    broadcast_guests_update(&state).await;

    Ok(Json(JoinResponse {
        guest_token: guest_id,
        name,
    }))
}

// ─── GET /api/search?q=&token= ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SearchParams {
    q: String,
    token: Uuid,
}

pub async fn search(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<crate::state::Track>>, (StatusCode, String)> {
    if !state.guests.read().await.contains_key(&params.token) {
        return Err((StatusCode::UNAUTHORIZED, "Invalid guest token".into()));
    }

    let access_token = {
        let spotify = state.spotify.read().await;
        match &*spotify {
            Some(auth) => auth.access_token.clone(),
            None => {
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Spotify not connected".into(),
                ))
            }
        }
    };

    let tracks = api::search_tracks(&params.q, &access_token)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(tracks))
}

// ─── POST /api/request ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RequestBody {
    track_id: String,
    guest_token: Uuid,
}

pub async fn request_track(
    State(state): State<AppState>,
    Json(body): Json<RequestBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    let guest_id = body.guest_token;

    if !state.guests.read().await.contains_key(&guest_id) {
        return Err((StatusCode::UNAUTHORIZED, "Invalid guest token".into()));
    }

    let settings = state.config.read().await.party_settings.clone();

    {
        let queue = state.queue.read().await;

        if queue.iter().any(|e| e.track.id == body.track_id) {
            return Err((StatusCode::CONFLICT, "Track already in queue".into()));
        }

        if settings.max_queue_size > 0 && queue.len() as u32 >= settings.max_queue_size {
            return Err((StatusCode::CONFLICT, format!("Queue is full (max {})", settings.max_queue_size)));
        }

        if settings.requests_per_guest > 0 {
            let guest_count = queue.iter().filter(|e| e.requested_by == guest_id).count();
            if guest_count as u32 >= settings.requests_per_guest {
                return Err((StatusCode::CONFLICT, format!("You can only have {} song(s) in the queue at once", settings.requests_per_guest)));
            }
        }
    }

    let access_token = {
        let spotify = state.spotify.read().await;
        match &*spotify {
            Some(auth) => auth.access_token.clone(),
            None => {
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Spotify not connected".into(),
                ))
            }
        }
    };

    let track = api::get_track(&body.track_id, &access_token)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if settings.block_explicit && track.explicit {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, "Explicit tracks are not allowed in this party".into()));
    }

    // Push to Spotify's actual queue immediately so the song plays without
    // the host needing to press "Play Next".
    let device_id = state.active_device_id.read().await.clone();
    if let Err(e) = api::add_to_spotify_queue(&track.uri, device_id.as_deref(), &access_token).await {
        eprintln!("Warning: could not auto-push track to Spotify queue: {}", e);
    }

    let entry = QueueEntry {
        track,
        votes: 0,
        requested_by: guest_id,
        requested_at: current_time(),
    };

    {
        let mut queue = state.queue.write().await;
        queue.push(entry);
        queue.sort_by(|a, b| {
            b.votes
                .cmp(&a.votes)
                .then(a.requested_at.cmp(&b.requested_at))
        });
    }

    broadcast_queue_update(&state).await;

    // ── Auto-skip check ────────────────────────────────────────────────────
    let settings = state.config.read().await.party_settings.clone();
    if settings.auto_skip_enabled {
        // Only trigger if the voted track is the currently playing one.
        let is_current = state
            .playback_cache
            .read()
            .await
            .as_ref()
            .and_then(|pb| pb.track_id.as_deref().map(|id| id == body.track_id))
            .unwrap_or(false);

        if is_current {
            let downvotes = state
                .votes
                .read()
                .await
                .iter()
                .filter(|((tid, _), &v)| tid == &body.track_id && v < 0)
                .count();

            let guest_count = state.guests.read().await.len();

            let should_skip = if settings.auto_skip_mode == "percentage" {
                guest_count > 0
                    && (downvotes as f32 / guest_count as f32 * 100.0)
                        >= settings.auto_skip_threshold
            } else {
                downvotes as f32 >= settings.auto_skip_threshold
            };

            if should_skip {
                let access_token = state
                    .spotify
                    .read()
                    .await
                    .as_ref()
                    .map(|a| a.access_token.clone());
                let device_id = state.active_device_id.read().await.clone();
                if let Some(token) = access_token {
                    let _ = api::skip_next(device_id.as_deref(), &token).await;
                }
            }
        }
    }

    Ok(StatusCode::OK)
}

// ─── POST /api/vote ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct VoteBody {
    track_id: String,
    /// "up" or "down"
    direction: String,
    guest_token: Uuid,
}

pub async fn vote(
    State(state): State<AppState>,
    Json(body): Json<VoteBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    let guest_id = body.guest_token;

    if !state.guests.read().await.contains_key(&guest_id) {
        return Err((StatusCode::UNAUTHORIZED, "Invalid guest token".into()));
    }

    let vote_value: i8 = match body.direction.as_str() {
        "up" => 1,
        "down" => -1,
        _ => return Err((StatusCode::BAD_REQUEST, "direction must be up or down".into())),
    };

    let key = (body.track_id.clone(), guest_id);

    // Calculate the net delta to apply to the vote count
    let delta: i32 = {
        let mut votes = state.votes.write().await;
        match votes.get(&key).copied() {
            Some(existing) if existing == vote_value => {
                // Toggle the same vote off
                votes.remove(&key);
                -(vote_value as i32)
            }
            Some(old) => {
                // Flip vote direction
                votes.insert(key, vote_value);
                (vote_value as i32) - (old as i32)
            }
            None => {
                // New vote
                votes.insert(key, vote_value);
                vote_value as i32
            }
        }
        // votes lock released here
    };

    {
        let mut queue = state.queue.write().await;
        if let Some(entry) = queue.iter_mut().find(|e| e.track.id == body.track_id) {
            entry.votes += delta;
        }
        queue.sort_by(|a, b| {
            b.votes
                .cmp(&a.votes)
                .then(a.requested_at.cmp(&b.requested_at))
        });
    }

    broadcast_queue_update(&state).await;
    Ok(StatusCode::OK)
}
