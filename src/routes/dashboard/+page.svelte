<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { onMount, onDestroy } from 'svelte';
  import QRCode from 'qrcode';

  interface Track {
    id: string;
    uri: string;
    title: string;
    artist: string;
    album: string;
    album_art_url: string | null;
    duration_ms: number;
  }

  interface QueueEntry {
    track: Track;
    votes: number;
    requested_by: string;
    requested_at: number;
  }

  interface Guest {
    id: string;
    name: string;
    joined_at: number;
  }

  interface PlaybackState {
    is_playing: boolean;
    track_name: string | null;
    artist_name: string | null;
    album_art_url: string | null;
    progress_ms: number | null;
    duration_ms: number | null;
  }

  // ── State ──────────────────────────────────────────────────────────────────
  let partyActive = $state(false);
  let tunnelUrl = $state('');
  let localUrl = $state('');
  let qrDataUrl = $state('');
  let queue = $state<QueueEntry[]>([]);
  let guests = $state<Guest[]>([]);
  let playback = $state<PlaybackState | null>(null);
  let loading = $state(false);
  let error = $state('');
  let playNextLoading = $state(false);
  let updateVersion = $state<string | null>(null);
  let updateInstalling = $state(false);

  // ── Lifecycle ──────────────────────────────────────────────────────────────
  let pollInterval: ReturnType<typeof setInterval>;

  onMount(async () => {
    // Silently check for updates in the background
    invoke<string | null>('check_for_updates').then(v => { updateVersion = v; }).catch(() => {});
    // Restore party state if already active
    const url = await invoke<string | null>('get_tunnel_url');
    if (url) {
      partyActive = true;
      tunnelUrl = url;
      await generateQr(url);
    }

    await refreshQueue();
    await refreshGuests();
    await refreshPlayback();

    pollInterval = setInterval(async () => {
      if (partyActive) {
        await refreshQueue();
        await refreshGuests();
        await refreshPlayback();
      }
    }, 5000);
  });

  onDestroy(() => {
    clearInterval(pollInterval);
  });

  // ── QR Code ────────────────────────────────────────────────────────────────
  async function generateQr(url: string) {
    try {
      qrDataUrl = await QRCode.toDataURL(url, {
        width: 200,
        margin: 2,
        color: { dark: '#000000', light: '#ffffff' },
      });
    } catch (e) {
      console.error('QR generation failed', e);
    }
  }

  // ── Party control ──────────────────────────────────────────────────────────
  async function startParty() {
    error = '';
    loading = true;
    try {
      const result = await invoke<{ tunnel_url: string; local_url: string }>('start_party');
      tunnelUrl = result.tunnel_url;
      localUrl = result.local_url;
      partyActive = true;
      await generateQr(tunnelUrl);
    } catch (e: unknown) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  async function stopParty() {
    if (!confirm('Stop the party? All guests will be disconnected.')) return;
    await invoke('stop_party');
    partyActive = false;
    tunnelUrl = '';
    qrDataUrl = '';
    queue = [];
    guests = [];
  }

  // ── Queue & playback ───────────────────────────────────────────────────────
  async function refreshQueue() {
    try { queue = await invoke<QueueEntry[]>('get_queue'); } catch {}
  }

  async function refreshGuests() {
    try { guests = await invoke<Guest[]>('get_guests'); } catch {}
  }

  async function refreshPlayback() {
    try { playback = await invoke<PlaybackState | null>('get_playback'); } catch {}
  }

  async function playNext() {
    error = '';
    playNextLoading = true;
    try {
      await invoke('play_next');
      await refreshQueue();
    } catch (e: unknown) {
      error = String(e);
    } finally {
      playNextLoading = false;
    }
  }

  async function removeTrack(trackId: string) {
    try {
      await invoke('remove_from_queue', { trackId });
      await refreshQueue();
    } catch (e: unknown) {
      error = String(e);
    }
  }

  // ── Helpers ────────────────────────────────────────────────────────────────
  function fmtDuration(ms: number) {
    const s = Math.floor(ms / 1000);
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  }

  function copyUrl() {
    navigator.clipboard.writeText(tunnelUrl);
  }

  async function installUpdate() {
    updateInstalling = true;
    try {
      await invoke('install_update'); // restarts the app on success
    } catch (e) {
      console.error('Update failed', e);
      updateInstalling = false;
    }
  }
</script>

<div class="root">
  {#if updateVersion}
    <div class="update-bar">
      <span>Update available: <strong>v{updateVersion}</strong></span>
      <button class="update-btn" onclick={installUpdate} disabled={updateInstalling}>
        {updateInstalling ? 'Installing…' : 'Install & Restart'}
      </button>
      <button class="update-dismiss" onclick={() => updateVersion = null} aria-label="Dismiss">✕</button>
    </div>
  {/if}

<div class="layout">
  <!-- ── Left panel: share + guests ── -->
  <aside class="sidebar">
    <div class="logo">Spar<span>tify</span></div>

    {#if !partyActive}
      <div class="start-section">
        <p class="hint">Start a party to get a shareable link for your guests.</p>
        {#if error}
          <div class="error">{error}</div>
        {/if}
        <button class="btn-primary big" onclick={startParty} disabled={loading}>
          {#if loading}
            <span class="spinner"></span> Starting…
          {:else}
            🎉 Start Party
          {/if}
        </button>
      </div>
    {:else}
      <!-- QR + URL -->
      <div class="share-card">
        {#if qrDataUrl}
          <img class="qr" src={qrDataUrl} alt="QR code for guests" />
        {/if}
        <div class="url-row">
          <span class="url-text" title={tunnelUrl}>{tunnelUrl}</span>
          <button class="icon-btn" onclick={copyUrl} title="Copy URL">⎘</button>
        </div>
        <p class="hint">Guests scan this QR or open the URL to join.</p>
      </div>

      <button class="btn-stop" onclick={stopParty}>■ Stop Party</button>
    {/if}

    <!-- Now playing -->
    {#if playback}
      <div class="now-playing">
        <div class="np-label">Now Playing</div>
        <div class="np-body">
          {#if playback.album_art_url}
            <img class="np-art" src={playback.album_art_url} alt="" />
          {/if}
          <div class="np-info">
            <div class="np-track">{playback.track_name ?? '—'}</div>
            <div class="np-artist">{playback.artist_name ?? ''}</div>
            {#if playback.is_playing}
              <div class="playing-badge">▶ Playing</div>
            {:else}
              <div class="playing-badge paused">⏸ Paused</div>
            {/if}
          </div>
        </div>
      </div>
    {:else}
      <div class="now-playing inactive">
        <div class="np-label">Now Playing</div>
        <p class="hint">Open Spotify on any device and start playing to enable queue control.</p>
      </div>
    {/if}

    <!-- Guests -->
    <div class="guests-section">
      <div class="section-header">
        Guests
        <span class="badge">{guests.length}</span>
      </div>
      {#if guests.length === 0}
        <p class="hint">No guests yet</p>
      {:else}
        <div class="guest-list">
          {#each guests as guest (guest.id)}
            <div class="guest-chip">{guest.name}</div>
          {/each}
        </div>
      {/if}
    </div>
  </aside>

  <!-- ── Right panel: queue ── -->
  <main class="queue-panel">
    <div class="queue-header">
      <div>
        <h1>Queue</h1>
        <span class="track-count">{queue.length} track{queue.length !== 1 ? 's' : ''}</span>
      </div>
      <div class="queue-actions">
        {#if error}
          <span class="error-inline">{error}</span>
        {/if}
        <button
          class="btn-primary"
          onclick={playNext}
          disabled={playNextLoading || queue.length === 0}
        >
          {playNextLoading ? '…' : '▶ Play Next'}
        </button>
      </div>
    </div>

    {#if queue.length === 0}
      <div class="empty-queue">
        <div class="empty-icon">🎵</div>
        {#if partyActive}
          <p>Waiting for guests to add songs…</p>
          <p class="hint">Share the QR code to get the party started!</p>
        {:else}
          <p>Start a party to let guests add songs.</p>
        {/if}
      </div>
    {:else}
      <div class="queue-list">
        {#each queue as entry, i (entry.track.id)}
          <div class="queue-item" class:top={i === 0}>
            <div class="pos">{i + 1}</div>

            {#if entry.track.album_art_url}
              <img class="art" src={entry.track.album_art_url} alt="" />
            {:else}
              <div class="art placeholder">♪</div>
            {/if}

            <div class="track-info">
              <div class="title">{entry.track.title}</div>
              <div class="sub">{entry.track.artist} · {entry.track.album}</div>
            </div>

            <div class="meta">
              <div
                class="votes"
                class:positive={entry.votes > 0}
                class:negative={entry.votes < 0}
              >
                {entry.votes > 0 ? '+' : ''}{entry.votes} votes
              </div>
              <div class="duration">{fmtDuration(entry.track.duration_ms)}</div>
            </div>

            <button
              class="remove-btn"
              onclick={() => removeTrack(entry.track.id)}
              title="Remove from queue"
            >✕</button>
          </div>
        {/each}
      </div>
    {/if}
  </main>
</div>
</div>

<style>
  .update-bar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 9px 20px;
    background: #1a3a28;
    border-bottom: 1px solid #1db954;
    font-size: 0.85rem;
    color: #ccc;
    flex-shrink: 0;
  }

  .update-bar span { flex: 1; }
  .update-bar strong { color: #1db954; }

  .update-btn {
    background: #1db954;
    color: #000;
    border: none;
    border-radius: 6px;
    padding: 6px 14px;
    font-size: 0.82rem;
    font-weight: 700;
    cursor: pointer;
    white-space: nowrap;
  }
  .update-btn:disabled { opacity: 0.6; cursor: not-allowed; }

  .update-dismiss {
    background: none;
    border: none;
    color: #777;
    cursor: pointer;
    font-size: 0.85rem;
    padding: 2px 6px;
    border-radius: 4px;
    flex-shrink: 0;
  }
  .update-dismiss:hover { color: #fff; }

  .root {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }

  .layout {
    display: flex;
    flex: 1;
    overflow: hidden;
  }

  /* ── Sidebar ── */
  .sidebar {
    width: 280px;
    min-width: 280px;
    background: #1e1e1e;
    border-right: 1px solid #2a2a2a;
    display: flex;
    flex-direction: column;
    gap: 20px;
    padding: 24px 20px;
    overflow-y: auto;
  }

  .logo {
    font-size: 1.6rem;
    font-weight: 800;
    letter-spacing: -0.5px;
    color: #fff;
    flex-shrink: 0;
  }
  .logo span { color: #1db954; }

  .hint {
    color: #777;
    font-size: 0.82rem;
    line-height: 1.5;
  }

  .start-section { display: flex; flex-direction: column; gap: 12px; }

  .error {
    background: rgba(231,76,60,0.12);
    border: 1px solid rgba(231,76,60,0.3);
    border-radius: 6px;
    padding: 8px 12px;
    font-size: 0.82rem;
    color: #e74c3c;
  }

  /* Share card */
  .share-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    background: #282828;
    border-radius: 10px;
    padding: 16px;
  }

  .qr {
    width: 160px;
    height: 160px;
    border-radius: 6px;
    display: block;
  }

  .url-row {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    background: #1e1e1e;
    border-radius: 6px;
    padding: 6px 10px;
  }

  .url-text {
    flex: 1;
    font-size: 0.75rem;
    color: #1db954;
    font-family: monospace;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .icon-btn {
    background: none;
    border: none;
    color: #b3b3b3;
    cursor: pointer;
    font-size: 1rem;
    padding: 2px 4px;
    border-radius: 4px;
    flex-shrink: 0;
  }
  .icon-btn:hover { color: #fff; background: #333; }

  /* Now playing */
  .now-playing {
    background: #282828;
    border-radius: 10px;
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .now-playing.inactive { opacity: 0.6; }

  .np-label { font-size: 0.72rem; font-weight: 700; text-transform: uppercase; letter-spacing: 1px; color: #777; }

  .np-body { display: flex; gap: 10px; align-items: flex-start; }

  .np-art { width: 48px; height: 48px; border-radius: 4px; object-fit: cover; flex-shrink: 0; }

  .np-info { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }

  .np-track { font-size: 0.88rem; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .np-artist { font-size: 0.78rem; color: #b3b3b3; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }

  .playing-badge {
    font-size: 0.72rem;
    color: #1db954;
    font-weight: 600;
    margin-top: 2px;
  }
  .playing-badge.paused { color: #777; }

  /* Guests */
  .guests-section { display: flex; flex-direction: column; gap: 8px; }

  .section-header {
    font-size: 0.75rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 1px;
    color: #777;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .badge {
    background: #333;
    color: #b3b3b3;
    border-radius: 10px;
    padding: 1px 7px;
    font-size: 0.72rem;
  }

  .guest-list { display: flex; flex-wrap: wrap; gap: 6px; }

  .guest-chip {
    background: #282828;
    border-radius: 20px;
    padding: 4px 10px;
    font-size: 0.8rem;
    color: #ccc;
  }

  /* Buttons */
  .btn-primary {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    background: #1db954;
    color: #000;
    border: none;
    border-radius: 8px;
    padding: 10px 16px;
    font-size: 0.9rem;
    font-weight: 700;
    cursor: pointer;
    transition: background 0.15s, opacity 0.15s;
  }
  .btn-primary.big { padding: 13px 16px; font-size: 0.95rem; }
  .btn-primary:hover:not(:disabled) { background: #17a349; }
  .btn-primary:disabled { opacity: 0.45; cursor: not-allowed; }

  .btn-stop {
    background: transparent;
    border: 1px solid #444;
    color: #b3b3b3;
    border-radius: 8px;
    padding: 8px 14px;
    font-size: 0.85rem;
    font-weight: 600;
    cursor: pointer;
    transition: border-color 0.15s, color 0.15s;
  }
  .btn-stop:hover { border-color: #e74c3c; color: #e74c3c; }

  .spinner {
    width: 16px;
    height: 16px;
    border: 2px solid rgba(0,0,0,0.2);
    border-top-color: #000;
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
    display: inline-block;
  }

  @keyframes spin { to { transform: rotate(360deg); } }

  /* ── Queue panel ── */
  .queue-panel {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: #121212;
  }

  .queue-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 24px 28px 16px;
    border-bottom: 1px solid #1e1e1e;
    gap: 12px;
    flex-shrink: 0;
  }

  .queue-header h1 {
    font-size: 1.4rem;
    font-weight: 800;
    color: #fff;
  }

  .track-count {
    font-size: 0.8rem;
    color: #666;
    margin-left: 8px;
  }

  .queue-actions {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .error-inline { font-size: 0.82rem; color: #e74c3c; max-width: 200px; }

  /* Empty state */
  .empty-queue {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    color: #555;
    font-size: 0.9rem;
    text-align: center;
    padding: 40px;
  }

  .empty-icon { font-size: 3rem; margin-bottom: 8px; }

  /* Queue list */
  .queue-list {
    flex: 1;
    overflow-y: auto;
    padding: 12px 20px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .queue-item {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 14px;
    border-radius: 8px;
    background: #1a1a1a;
    transition: background 0.15s;
  }
  .queue-item:hover { background: #222; }
  .queue-item.top { border-left: 3px solid #1db954; }

  .pos {
    width: 24px;
    text-align: center;
    font-size: 0.8rem;
    color: #555;
    font-weight: 700;
    flex-shrink: 0;
  }

  .art {
    width: 44px;
    height: 44px;
    border-radius: 4px;
    object-fit: cover;
    flex-shrink: 0;
  }

  .art.placeholder {
    background: #282828;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #555;
    font-size: 1.1rem;
  }

  .track-info { flex: 1; min-width: 0; }

  .title {
    font-size: 0.9rem;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .sub {
    font-size: 0.78rem;
    color: #777;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    margin-top: 2px;
  }

  .meta {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 3px;
    flex-shrink: 0;
  }

  .votes {
    font-size: 0.78rem;
    font-weight: 600;
    color: #666;
  }
  .votes.positive { color: #1db954; }
  .votes.negative { color: #e74c3c; }

  .duration { font-size: 0.75rem; color: #555; }

  .remove-btn {
    background: none;
    border: none;
    color: #444;
    cursor: pointer;
    font-size: 0.9rem;
    padding: 4px 6px;
    border-radius: 4px;
    flex-shrink: 0;
    transition: color 0.15s, background 0.15s;
  }
  .remove-btn:hover { color: #e74c3c; background: rgba(231,76,60,0.1); }
</style>
