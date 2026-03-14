use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::Rng;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// The custom URI scheme registered in tauri.conf.json.
/// Users register exactly this string in their Spotify Developer Dashboard.
pub const REDIRECT_URI: &str = "spartify://callback";

pub fn current_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn generate_pkce() -> (String, String) {
    let bytes: Vec<u8> = (0..32).map(|_| rand::thread_rng().gen()).collect();
    let code_verifier = URL_SAFE_NO_PAD.encode(&bytes);

    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    let code_challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());

    (code_verifier, code_challenge)
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
}

/// Launches the PKCE OAuth flow via a custom deep link URI scheme.
///
/// 1. Stores a oneshot sender in `state.oauth_code_tx`
/// 2. Opens the Spotify auth URL in the user's browser
/// 3. When the user authorises, Spotify redirects to `spartify://callback?code=...`
/// 4. The OS routes that deep link back to this app; the deep link handler in
///    `lib.rs` extracts the code and fires the sender
/// 5. We receive the code here and exchange it for tokens
///
/// Returns `(access_token, refresh_token, expires_at)`.
pub async fn start_oauth_flow(
    client_id: &str,
    state: &crate::state::AppState,
    app_handle: &tauri::AppHandle,
) -> anyhow::Result<(String, String, u64)> {
    let (code_verifier, code_challenge) = generate_pkce();

    // Place the sender in AppState so the deep link handler can reach it
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    *state.oauth_code_tx.lock().await = Some(tx);

    let scopes =
        "user-read-playback-state user-modify-playback-state user-read-currently-playing";

    let auth_url = reqwest::Url::parse_with_params(
        "https://accounts.spotify.com/authorize",
        &[
            ("client_id", client_id),
            ("response_type", "code"),
            ("redirect_uri", REDIRECT_URI),
            ("scope", scopes),
            ("code_challenge_method", "S256"),
            ("code_challenge", code_challenge.as_str()),
        ],
    )?;

    use tauri_plugin_opener::OpenerExt;
    app_handle
        .opener()
        .open_url(auth_url.as_str(), None::<String>)?;

    // Wait up to 5 minutes for the deep link callback to fire
    let code = tokio::time::timeout(std::time::Duration::from_secs(300), rx)
        .await
        .map_err(|_| anyhow::anyhow!("OAuth timed out after 5 minutes"))?
        .map_err(|_| anyhow::anyhow!("OAuth callback channel closed unexpectedly"))?;

    let client = reqwest::Client::new();
    let response = client
        .post("https://accounts.spotify.com/api/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", client_id),
            ("code_verifier", code_verifier.as_str()),
        ])
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Token exchange failed ({}): {}", status, body);
    }

    let token_data: TokenResponse = response.json().await?;
    let expires_at = current_time_secs() + token_data.expires_in.saturating_sub(60);

    let refresh_token = token_data
        .refresh_token
        .ok_or_else(|| anyhow::anyhow!("Spotify did not return a refresh token"))?;

    Ok((token_data.access_token, refresh_token, expires_at))
}

/// Uses the stored refresh token to obtain a new access token.
/// Returns `(access_token, expires_at)`.
pub async fn refresh_access_token(
    client_id: &str,
    refresh_token: &str,
) -> anyhow::Result<(String, u64)> {
    #[derive(Deserialize)]
    struct RefreshResponse {
        access_token: String,
        expires_in: u64,
    }

    let client = reqwest::Client::new();
    let response = client
        .post("https://accounts.spotify.com/api/token")
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ])
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Token refresh failed ({}): {}", status, body);
    }

    let data: RefreshResponse = response.json().await?;
    let expires_at = current_time_secs() + data.expires_in.saturating_sub(60);

    Ok((data.access_token, expires_at))
}
