use tauri_plugin_shell::ShellExt;
use tauri_plugin_shell::process::CommandEvent;

/// Spawns the bundled `bore` sidecar as `bore local <port> --to bore.pub`
/// and waits for it to report the public URL.
/// Returns `(public_url, child_process)`.
pub async fn start_bore_tunnel(
    app: &tauri::AppHandle,
    port: u16,
) -> anyhow::Result<(String, tauri_plugin_shell::process::CommandChild)> {
    let (mut rx, child) = app
        .shell()
        .sidecar("bore")
        .map_err(|e| anyhow::anyhow!("Failed to find bore sidecar: {}", e))?
        .args(["local", &port.to_string(), "--to", "bore.pub"])
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to launch bore: {}", e))?;

    // Wait up to 20 seconds for bore to print its "listening at bore.pub:PORT" line.
    let url = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        async {
            while let Some(event) = rx.recv().await {
                match event {
                    CommandEvent::Stdout(bytes) | CommandEvent::Stderr(bytes) => {
                        let line = String::from_utf8_lossy(&bytes);
                        if let Some(url) = parse_bore_url(&line) {
                            // Drain remaining output in the background so bore
                            // never blocks on a full pipe buffer.
                            tokio::spawn(async move {
                                while rx.recv().await.is_some() {}
                            });
                            return Ok(url);
                        }
                    }
                    CommandEvent::Error(e) => {
                        return Err(anyhow::anyhow!("bore error: {}", e));
                    }
                    CommandEvent::Terminated(s) => {
                        return Err(anyhow::anyhow!(
                            "bore exited early (code {:?})",
                            s.code
                        ));
                    }
                    _ => {}
                }
            }
            Err(anyhow::anyhow!("bore output ended without a tunnel URL"))
        },
    )
    .await
    .map_err(|_| anyhow::anyhow!("Timed out waiting for bore tunnel (20s)"))??;

    Ok((url, child))
}

/// Parses a line like `… listening at bore.pub:12345` → `"http://bore.pub:12345"`.
fn parse_bore_url(line: &str) -> Option<String> {
    let needle = "bore.pub:";
    let idx = line.find(needle)?;
    let rest = &line[idx + needle.len()..];
    let port_str = rest.split_whitespace().next()?;
    port_str.parse::<u16>().ok()?;
    Some(format!("http://bore.pub:{}", port_str))
}
