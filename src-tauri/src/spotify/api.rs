use serde::{Deserialize, Serialize};

use crate::state::{PlaybackState, Track};

// ─── Search ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SearchResponse {
    tracks: TrackPage,
}

#[derive(Deserialize)]
struct TrackPage {
    items: Vec<SpotifyTrack>,
}

#[derive(Deserialize)]
struct SpotifyTrack {
    id: String,
    uri: String,
    name: String,
    artists: Vec<SpotifyArtist>,
    album: SpotifyAlbum,
    duration_ms: u64,
}

#[derive(Deserialize)]
struct SpotifyArtist {
    name: String,
}

#[derive(Deserialize)]
struct SpotifyAlbum {
    name: String,
    images: Vec<SpotifyImage>,
}

#[derive(Deserialize)]
struct SpotifyImage {
    url: String,
    width: Option<u32>,
}

pub async fn search_tracks(query: &str, access_token: &str) -> anyhow::Result<Vec<Track>> {
    let client = reqwest::Client::new();
    let response = client
        .get("https://api.spotify.com/v1/search")
        .query(&[("q", query), ("type", "track"), ("limit", "10")])
        .bearer_auth(access_token)
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("Spotify search failed: {}", response.status());
    }

    let data: SearchResponse = response.json().await?;
    let tracks = data
        .tracks
        .items
        .into_iter()
        .map(|t| {
            let artist = t
                .artists
                .iter()
                .map(|a| a.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            // Prefer a ~300px image; fall back to the first one
            let album_art_url = t
                .album
                .images
                .iter()
                .find(|img| img.width.map_or(false, |w| w <= 300))
                .or_else(|| t.album.images.first())
                .map(|img| img.url.clone());
            Track {
                id: t.id,
                uri: t.uri,
                title: t.name,
                artist,
                album: t.album.name,
                album_art_url,
                duration_ms: t.duration_ms,
            }
        })
        .collect();

    Ok(tracks)
}

// ─── Get single track by ID ───────────────────────────────────────────────────

pub async fn get_track(track_id: &str, access_token: &str) -> anyhow::Result<Track> {
    #[derive(Deserialize)]
    struct SingleTrack {
        id: String,
        uri: String,
        name: String,
        artists: Vec<SpotifyArtist>,
        album: SpotifyAlbum,
        duration_ms: u64,
    }

    let client = reqwest::Client::new();
    let response = client
        .get(format!("https://api.spotify.com/v1/tracks/{}", track_id))
        .bearer_auth(access_token)
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("Failed to fetch track {}: {}", track_id, response.status());
    }

    let t: SingleTrack = response.json().await?;
    let artist = t
        .artists
        .iter()
        .map(|a| a.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let album_art_url = t
        .album
        .images
        .iter()
        .find(|img| img.width.map_or(false, |w| w <= 300))
        .or_else(|| t.album.images.first())
        .map(|img| img.url.clone());

    Ok(Track {
        id: t.id,
        uri: t.uri,
        title: t.name,
        artist,
        album: t.album.name,
        album_art_url,
        duration_ms: t.duration_ms,
    })
}

// ─── Queue ────────────────────────────────────────────────────────────────────

/// Appends a track URI to the user's Spotify queue.
/// `device_id` optionally routes the request to a specific Spotify Connect device.
pub async fn add_to_spotify_queue(
    track_uri: &str,
    device_id: Option<&str>,
    access_token: &str,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let mut req = client
        .post("https://api.spotify.com/v1/me/player/queue")
        .query(&[("uri", track_uri)])
        .bearer_auth(access_token)
        .header("Content-Length", "0");
    if let Some(id) = device_id {
        req = req.query(&[("device_id", id)]);
    }
    let response = req.send().await?;

    match response.status().as_u16() {
        200 | 204 => Ok(()),
        404 => anyhow::bail!(
            "No active Spotify device found. Open Spotify on any device and start playing first."
        ),
        403 => anyhow::bail!("Spotify Premium is required to control playback."),
        _ => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to add to Spotify queue ({}): {}", status, body)
        }
    }
}

// ─── Devices ─────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Device {
    pub id: Option<String>,
    pub name: String,
    pub device_type: String,
    pub is_active: bool,
    pub volume_percent: Option<u32>,
}

pub async fn get_devices(access_token: &str) -> anyhow::Result<Vec<Device>> {
    #[derive(Deserialize)]
    struct RawDevice {
        id: Option<String>,
        name: String,
        #[serde(rename = "type")]
        device_type: String,
        is_active: bool,
        volume_percent: Option<u32>,
    }

    #[derive(Deserialize)]
    struct DevicesResponse {
        devices: Vec<RawDevice>,
    }

    let client = reqwest::Client::new();
    let response = client
        .get("https://api.spotify.com/v1/me/player/devices")
        .bearer_auth(access_token)
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("Failed to get devices: {}", response.status());
    }

    let data: DevicesResponse = response.json().await?;
    Ok(data
        .devices
        .into_iter()
        .map(|d| Device {
            id: d.id,
            name: d.name,
            device_type: d.device_type,
            is_active: d.is_active,
            volume_percent: d.volume_percent,
        })
        .collect())
}

/// Transfers playback to the given device.
/// `play: true` starts playback immediately; `false` transfers without interrupting.
pub async fn transfer_playback(
    device_id: &str,
    play: bool,
    access_token: &str,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .put("https://api.spotify.com/v1/me/player")
        .bearer_auth(access_token)
        .json(&serde_json::json!({ "device_ids": [device_id], "play": play }))
        .send()
        .await?;

    match response.status().as_u16() {
        200 | 202 | 204 => Ok(()),
        404 => anyhow::bail!("Device not found or no active session."),
        403 => anyhow::bail!("Spotify Premium is required to transfer playback."),
        _ => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Transfer playback failed ({}): {}", status, body)
        }
    }
}

pub async fn get_playback_state(
    access_token: &str,
) -> anyhow::Result<Option<PlaybackState>> {
    #[derive(Deserialize)]
    struct RawState {
        is_playing: bool,
        progress_ms: Option<u64>,
        item: Option<RawItem>,
        device: Option<RawDevice>,
    }

    #[derive(Deserialize)]
    struct RawItem {
        id: String,
        name: String,
        duration_ms: u64,
        artists: Vec<SpotifyArtist>,
        album: SpotifyAlbum,
    }

    #[derive(Deserialize)]
    struct RawDevice {
        name: String,
    }

    let client = reqwest::Client::new();
    let response = client
        .get("https://api.spotify.com/v1/me/player")
        .bearer_auth(access_token)
        .send()
        .await?;

    if response.status() == 204 {
        return Ok(None);
    }

    if !response.status().is_success() {
        anyhow::bail!("Failed to get playback state: {}", response.status());
    }

    let data: RawState = response.json().await?;
    let (track_id, track_name, artist_name, album_art_url, duration_ms) = match data.item {
        Some(item) => {
            let artist = item
                .artists
                .iter()
                .map(|a| a.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            let art = item
                .album
                .images
                .iter()
                .find(|img| img.width.map_or(false, |w| w <= 300))
                .or_else(|| item.album.images.first())
                .map(|img| img.url.clone());
            (Some(item.id), Some(item.name), Some(artist), art, Some(item.duration_ms))
        }
        None => (None, None, None, None, None),
    };

    Ok(Some(PlaybackState {
        is_playing: data.is_playing,
        track_id,
        track_name,
        artist_name,
        album_art_url,
        progress_ms: data.progress_ms,
        duration_ms,
        device_name: data.device.map(|d| d.name),
    }))
}

// ─── Playback controls ────────────────────────────────────────────────────────

pub async fn set_playback(
    play: bool,
    device_id: Option<&str>,
    access_token: &str,
) -> anyhow::Result<()> {
    let endpoint = if play {
        "https://api.spotify.com/v1/me/player/play"
    } else {
        "https://api.spotify.com/v1/me/player/pause"
    };
    let client = reqwest::Client::new();
    let mut req = client
        .put(endpoint)
        .bearer_auth(access_token)
        .header("Content-Length", "0");
    if let Some(id) = device_id {
        req = req.query(&[("device_id", id)]);
    }
    let response = req.send().await?;
    match response.status().as_u16() {
        200 | 202 | 204 => Ok(()),
        403 => anyhow::bail!("Spotify Premium is required to control playback."),
        404 => anyhow::bail!("No active Spotify device."),
        _ => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Playback control failed ({}): {}", status, body)
        }
    }
}

pub async fn skip_next(device_id: Option<&str>, access_token: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let mut req = client
        .post("https://api.spotify.com/v1/me/player/next")
        .bearer_auth(access_token)
        .header("Content-Length", "0");
    if let Some(id) = device_id {
        req = req.query(&[("device_id", id)]);
    }
    let response = req.send().await?;
    match response.status().as_u16() {
        200 | 204 => Ok(()),
        403 => anyhow::bail!("Spotify Premium is required."),
        404 => anyhow::bail!("No active Spotify device."),
        _ => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Skip failed ({}): {}", status, body)
        }
    }
}

pub async fn skip_previous(device_id: Option<&str>, access_token: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let mut req = client
        .post("https://api.spotify.com/v1/me/player/previous")
        .bearer_auth(access_token)
        .header("Content-Length", "0");
    if let Some(id) = device_id {
        req = req.query(&[("device_id", id)]);
    }
    let response = req.send().await?;
    match response.status().as_u16() {
        200 | 204 => Ok(()),
        403 => anyhow::bail!("Spotify Premium is required."),
        404 => anyhow::bail!("No active Spotify device."),
        _ => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Previous failed ({}): {}", status, body)
        }
    }
}

/// Returns the tracks currently in Spotify's play queue (up next, not yet played).
pub async fn get_spotify_queue(access_token: &str) -> anyhow::Result<Vec<Track>> {
    #[derive(Deserialize)]
    struct QueueResponse {
        queue: Vec<SpotifyTrack>,
    }

    let client = reqwest::Client::new();
    let response = client
        .get("https://api.spotify.com/v1/me/player/queue")
        .bearer_auth(access_token)
        .send()
        .await?;

    if response.status() == 204 {
        return Ok(Vec::new());
    }

    if !response.status().is_success() {
        anyhow::bail!("Failed to get Spotify queue: {}", response.status());
    }

    let data: QueueResponse = response.json().await?;
    let tracks = data
        .queue
        .into_iter()
        .map(|t| {
            let artist = t
                .artists
                .iter()
                .map(|a| a.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            let album_art_url = t
                .album
                .images
                .iter()
                .find(|img| img.width.map_or(false, |w| w <= 300))
                .or_else(|| t.album.images.first())
                .map(|img| img.url.clone());
            Track {
                id: t.id,
                uri: t.uri,
                title: t.name,
                artist,
                album: t.album.name,
                album_art_url,
                duration_ms: t.duration_ms,
            }
        })
        .collect();
    Ok(tracks)
}
