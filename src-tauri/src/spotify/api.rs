use serde::{Deserialize, Serialize};

use crate::state::Track;

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
pub async fn add_to_spotify_queue(track_uri: &str, access_token: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .post("https://api.spotify.com/v1/me/player/queue")
        .query(&[("uri", track_uri)])
        .bearer_auth(access_token)
        .header("Content-Length", "0")
        .send()
        .await?;

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

// ─── Playback state ───────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PlaybackState {
    pub is_playing: bool,
    pub track_name: Option<String>,
    pub artist_name: Option<String>,
    pub album_art_url: Option<String>,
    pub progress_ms: Option<u64>,
    pub duration_ms: Option<u64>,
}

pub async fn get_playback_state(
    access_token: &str,
) -> anyhow::Result<Option<PlaybackState>> {
    #[derive(Deserialize)]
    struct RawState {
        is_playing: bool,
        progress_ms: Option<u64>,
        item: Option<RawItem>,
    }

    #[derive(Deserialize)]
    struct RawItem {
        name: String,
        duration_ms: u64,
        artists: Vec<SpotifyArtist>,
        album: SpotifyAlbum,
    }

    let client = reqwest::Client::new();
    let response = client
        .get("https://api.spotify.com/v1/me/player")
        .bearer_auth(access_token)
        .send()
        .await?;

    if response.status() == 204 {
        // No active device
        return Ok(None);
    }

    if !response.status().is_success() {
        anyhow::bail!("Failed to get playback state: {}", response.status());
    }

    let data: RawState = response.json().await?;
    let (track_name, artist_name, album_art_url, duration_ms) = match data.item {
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
            (Some(item.name), Some(artist), art, Some(item.duration_ms))
        }
        None => (None, None, None, None),
    };

    Ok(Some(PlaybackState {
        is_playing: data.is_playing,
        track_name,
        artist_name,
        album_art_url,
        progress_ms: data.progress_ms,
        duration_ms,
    }))
}
