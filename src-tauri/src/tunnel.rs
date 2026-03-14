use tokio::io::AsyncBufReadExt;

/// Spawns `bore local <port> --to bore.pub` and waits for it to report
/// the public URL. Returns (public_url, child_process).
///
/// Requires `bore` to be installed: `cargo install bore-cli`
pub async fn start_bore_tunnel(
    port: u16,
) -> anyhow::Result<(String, tokio::process::Child)> {
    let mut child = tokio::process::Command::new("bore")
        .args(["local", &port.to_string(), "--to", "bore.pub"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!(
                    "`bore` was not found on your PATH.\n\
                     Install it with:  cargo install bore-cli\n\
                     Then restart Spartify."
                )
            } else {
                anyhow::anyhow!("Failed to launch bore: {}", e)
            }
        })?;

    // bore writes its log lines to stderr
    let stderr = child.stderr.take().expect("bore stderr should be piped");
    let stdout = child.stdout.take().expect("bore stdout should be piped");

    let tunnel_url = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        wait_for_url(stderr, stdout),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Timed out waiting for bore tunnel to establish (20s)"))?
    .map_err(|e| anyhow::anyhow!("bore error: {}", e))?;

    Ok((tunnel_url, child))
}

async fn wait_for_url(
    stderr: tokio::process::ChildStderr,
    stdout: tokio::process::ChildStdout,
) -> anyhow::Result<String> {
    let mut stderr_lines = tokio::io::BufReader::new(stderr).lines();
    let mut stdout_lines = tokio::io::BufReader::new(stdout).lines();

    loop {
        tokio::select! {
            line = stderr_lines.next_line() => {
                if let Ok(Some(line)) = line {
                    if let Some(url) = parse_bore_url(&line) {
                        return Ok(url);
                    }
                }
            }
            line = stdout_lines.next_line() => {
                if let Ok(Some(line)) = line {
                    if let Some(url) = parse_bore_url(&line) {
                        return Ok(url);
                    }
                }
            }
        }
    }
}

/// Parses a line like:
///   `... listening at bore.pub:12345`
/// and returns `"http://bore.pub:12345"`.
fn parse_bore_url(line: &str) -> Option<String> {
    let needle = "bore.pub:";
    let idx = line.find(needle)?;
    let rest = &line[idx + needle.len()..];
    let port_str = rest.split_whitespace().next()?;
    port_str.parse::<u16>().ok()?;
    Some(format!("http://bore.pub:{}", port_str))
}
