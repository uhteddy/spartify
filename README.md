[![Spartify-(1250-x-500-px).png](https://i.postimg.cc/6Q7rVPWK/Spartify-(1250-x-500-px).png)](https://postimg.cc/R3BJ4XJp)

# Spartify

Spartify is an open-source macOS desktop app that turns your Spotify account into a party music station. Guests join from any device using a browser — no app required — and can search for songs, request them, and vote on what plays next. You stay in control the whole time.

---

## Features

### For the host
- **One-click party** — start a party and get a shareable URL and QR code instantly
- **Full playback controls** — play, pause, skip forward, skip back directly from Spartify
- **Now Playing display** — album art, track name, artist, and a live progress bar synced to Spotify
- **Party Queue** — see every guest-requested song in the order Spotify will actually play them
- **Spotify Autoplay** — see what Spotify has lined up after the party queue, deduplicated so nothing shows twice
- **Playback sync** — Spartify polls Spotify every 3 seconds to keep the queue order and playback state accurate across all views
- **Auto-queue** — guest requests are pushed to Spotify's queue immediately; no manual "play next" needed
- **Auto-retire** — when a requested song starts playing it is automatically removed from the party queue and added to history
- **Order sync** — if you or Spotify reorders tracks, the party queue display updates to match within 3 seconds
- **Track history** — a log of every song played during the party
- **Guest list** — see who has joined the party in real time
- **Device picker** — choose which Spotify Connect device to target for playback
- **Party settings** — configure the party without stopping it:
  - Optional join password
  - Max concurrent requests per guest
  - Max total queue size
  - Block explicit tracks
- **Auto-updater** — checks for new Spartify versions on launch and can install them in one click

### For guests
- **Browser-based** — join from any phone or laptop, no install required
- **Song search** — search Spotify's full catalog and request any track
- **Voting** — upvote or downvote songs in the queue; the order adjusts in real time
- **Now Playing** — see the current track with album art, artist, title, and a live progress bar
- **Party Queue** — see all requested songs and their vote counts
- **Spotify Autoplay** — see what else is coming up after the requested songs
- **History tab** — browse every track played so far in the party
- **Open in Spotify** — tap any track to open it in Spotify directly
- **Real-time updates** — everything is pushed over WebSocket so the view stays current without refreshing

---

## How it works (semi-technical)

Spartify is a [Tauri 2](https://tauri.app) desktop app: a Rust backend bundled with a [SvelteKit 5](https://kit.svelte.dev) frontend.

When you start a party, the Rust backend spins up an embedded [axum](https://github.com/tokio-rs/axum) HTTP server on a random local port. An [frp](https://github.com/fatedier/frp) tunnel is launched as a bundled sidecar binary, creating a publicly accessible subdomain URL (e.g. `abc123.spartify.app`) that forwards to that local server. Guests open this URL in any browser and get served a self-contained HTML page.

The guest page communicates with the backend over WebSocket for real-time pushes (queue changes, playback updates, new songs starting) and over plain HTTP for actions (joining, searching, requesting, voting).

Spotify integration uses the [PKCE OAuth flow](https://developer.spotify.com/documentation/web-api/tutorials/code-pkce-flow) so no client secret is ever stored. Spartify also registers itself as a [Spotify Web Playback SDK](https://developer.spotify.com/documentation/web-playback-sdk) virtual device so you can target it from Spotify Connect.

A background task runs every 3 seconds and:
1. Fetches the current Spotify playback state and broadcasts it to all connected guests
2. Fetches Spotify's upcoming queue, updates the Autoplay display, and reorders the Party Queue to match Spotify's actual playback order
3. Detects when a guest-requested song starts playing and automatically retires it from the queue into history

All party state (queue, guests, votes, history) lives in memory and is cleared when the party stops.

---

## Building locally

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (stable, via `rustup`)
- [Bun](https://bun.sh) (or Node.js 18+)
- [Tauri CLI prerequisites](https://tauri.app/start/prerequisites/) for macOS (Xcode Command Line Tools)
- A Spotify Developer app with:
  - A redirect URI of `spartify://callback` registered
  - The `streaming` scope enabled (required for Web Playback SDK)

### Steps

```sh
# 1. Clone the repo
git clone https://github.com/uhteddy/spartify.git
cd spartify

# 2. Install JS dependencies
bun install

# 3. Download the frp sidecar binary
# Download frp from https://github.com/fatedier/frp/releases and place the frpc binary at:
cp /path/to/frpc src-tauri/binaries/frpc-aarch64-apple-darwin
codesign -f -s - src-tauri/binaries/frpc-aarch64-apple-darwin

# 4. Run in development mode
bunx tauri dev
```

> **Apple Silicon vs Intel:** The sidecar binary name must match your architecture.
> - Apple Silicon: `frpc-aarch64-apple-darwin`
> - Intel Mac: `frpc-x86_64-apple-darwin`
> - Check yours with: `rustc -vV | grep host`

### Building a release `.dmg`

```sh
bunx tauri build
# Output: src-tauri/target/release/bundle/dmg/Spartify_*.dmg
```

### First launch

1. Open Spartify and enter your Spotify app's **Client ID**
2. Click **Connect Spotify** — this opens Spotify's OAuth page in your browser
3. After authorizing, you'll be redirected back and logged in
4. Click **Start Party** to generate your shareable URL and QR code

---

## FAQ

**Q: Why use this over Spotify Jams?**

Spotify Jams are great for small gatherings, but they give every participant equal control. Spartify is designed for hosted events — guests can request and vote on songs, but the host stays in full control of playback and can set limits (requests per person, queue size, no explicit tracks, password-protected entry).

**Q: Does Spartify require Spotify Premium?**

Yes. Spotify's playback control API and Web Playback SDK both require a Premium account for the host. Guests do not need any Spotify account.

**Q: Do guests need to install anything?**

No. Guests open a URL (or scan a QR code) in any modern mobile or desktop browser. Everything runs in the browser — no app, no login required.

**Q: Is the party link accessible from outside my home network?**

Yes. Spartify automatically creates a public tunnel to a subdomain at `spartify.app` so guests can join from anywhere, not just your local Wi-Fi.

**Q: How do I prevent randos from joining?**

Set a join password in the party settings (the gear icon). Guests will be prompted for the password before they can enter.

**Q: Can I limit how many songs each guest can request?**

Yes. In party settings you can set a maximum number of concurrent requests per guest. Once a guest's songs have played, their slot opens back up.

**Q: What happens if I skip a song manually in Spotify?**

Spartify detects the track change within ~3 seconds and retires the skipped song from the party queue automatically.

**Q: Does Spartify store any data?**

Spartify stores only your Spotify Client ID and refresh token locally in a config file (`~/Library/Application Support/spartify/config.json` on macOS). No data is sent to any Spartify server. Party state lives entirely in memory and is gone when the party stops.

**Q: Can I run Spartify on Windows or Linux?**

The codebase is cross-platform (Tauri supports Windows and Linux), but only macOS builds are currently tested and distributed. You can compile from source on other platforms — your mileage may vary.

**Q: How do I update Spartify?**

Spartify checks for updates automatically on launch. If a new version is available you'll see an update banner at the top of the window. Click **Update** to download and install it.

**Q: Does Spartify store any data about my Spotify account?**
No data is ever stored or processed anywhere but your device. When you authenticate you are logged in with your own Spotify Developer app credentials, on your own device. Your account token and refresh token are stored locally on your device. Spartify runs a tunnel to your local machine so guests can connect, but no data is sent to any external server. Party state (queue, guests, votes, history) lives entirely in memory and is cleared when the party stops.
