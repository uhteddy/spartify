use std::io::Write;
use tauri_plugin_shell::process::CommandEvent;
use tauri_plugin_shell::ShellExt;

/// The frp server address and shared secret baked into the binary.
/// The secret must match `auth.token` in frps.toml on your VPS.
const FRP_SERVER: &str = "tunnel.spartify.app";
const FRP_PORT: u16 = 7000;
const FRP_TOKEN: &str = "VlTCQb2HKoqMFJuo";
/// Literal security theater but it stops sniffers from being weird

/// Spawns the bundled `frpc` sidecar with the given subdomain (or a random one),
/// waits for it to report the tunnel is active, then returns the public URL.
/// Returns `(public_url, child_process)`.
pub async fn start_frp_tunnel(
    app: &tauri::AppHandle,
    local_port: u16,
    custom_subdomain: Option<String>,
) -> anyhow::Result<(String, tauri_plugin_shell::process::CommandChild)> {
    let subdomain: String = match custom_subdomain {
        Some(s) => {
            let s = s.trim().to_lowercase();
            if s.is_empty() || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                anyhow::bail!("Subdomain must contain only letters, numbers, or hyphens");
            }
            s
        }
        // Generate a random 8-char subdomain (lowercase alphanumeric).
        None => (0..8)
            .map(|_| {
                let idx = (rand_byte() % 36) as usize;
                b"abcdefghijklmnopqrstuvwxyz0123456789"[idx] as char
            })
            .collect(),
    };

    // Write a temporary frpc config file.
    let config_path = std::env::temp_dir().join(format!("frpc-{}.toml", subdomain));
    {
        let mut f = std::fs::File::create(&config_path)
            .map_err(|e| anyhow::anyhow!("Failed to write frpc config: {}", e))?;
        write!(
            f,
            r#"serverAddr = "{server}"
serverPort = {port}

auth.method = "token"
auth.token = "{token}"

[[proxies]]
name = "party-{sub}"
type = "http"
localPort = {local_port}
subdomain = "{sub}"
"#,
            server = FRP_SERVER,
            port = FRP_PORT,
            token = FRP_TOKEN,
            sub = subdomain,
            local_port = local_port,
        )
        .map_err(|e| anyhow::anyhow!("Failed to write frpc config: {}", e))?;
    }

    let config_str = config_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Config path is not valid UTF-8"))?
        .to_owned();

    let (mut rx, child) = app
        .shell()
        .sidecar("frpc")
        .map_err(|e| anyhow::anyhow!("Failed to find frpc sidecar: {}", e))?
        .args(["-c", &config_str])
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to launch frpc: {}", e))?;

    let public_url = format!("https://{}.spartify.app", subdomain);

    // Wait up to 20 seconds for frpc to confirm the tunnel is up.
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while let Some(event) = rx.recv().await {
            match event {
                CommandEvent::Stdout(bytes) | CommandEvent::Stderr(bytes) => {
                    let line = String::from_utf8_lossy(&bytes);
                    // frpc prints "start proxy success" when the tunnel is established.
                    if line.contains("start proxy success")
                        || line.contains("login to server success")
                    {
                        tokio::spawn(async move { while rx.recv().await.is_some() {} });
                        return Ok(());
                    }
                    if line.contains("login to server failed") || line.contains("EOF") {
                        return Err(anyhow::anyhow!("frpc failed to connect: {}", line.trim()));
                    }
                }
                CommandEvent::Terminated(s) => {
                    return Err(anyhow::anyhow!("frpc exited early (code {:?})", s.code));
                }
                _ => {}
            }
        }
        Err(anyhow::anyhow!(
            "frpc output ended without confirming tunnel"
        ))
    })
    .await
    .map_err(|_| anyhow::anyhow!("Timed out waiting for frpc tunnel (20s)"))??;

    // Clean up the temp config file (non-fatal if it fails).
    let _ = std::fs::remove_file(&config_path);

    Ok((public_url, child))
}

/// Simple non-crypto random byte using system time entropy.
/// Avoids adding a rand crate dependency.
fn rand_byte() -> u8 {
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    let c = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    ((t ^ (c.wrapping_mul(6364136223846793005))) >> 8) as u8
}
