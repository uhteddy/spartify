<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount, onDestroy } from 'svelte';
  import QRCode from 'qrcode';

  // ── Spotify Web Playback SDK types ─────────────────────────────────────────
  declare global {
    interface Window {
      Spotify: {
        Player: new (options: {
          name: string;
          getOAuthToken: (cb: (token: string) => void) => void;
          volume?: number;
        }) => SpotifyPlayer;
      };
      onSpotifyWebPlaybackSDKReady: () => void;
    }
  }

  interface SpotifyPlayer {
    connect(): Promise<boolean>;
    disconnect(): void;
    addListener(event: 'ready', cb: (data: { device_id: string }) => void): void;
    addListener(event: 'player_state_changed', cb: (state: SpotifyPlayerState | null) => void): void;
    addListener(event: string, cb: (...args: unknown[]) => void): void;
  }

  interface SpotifyPlayerState {
    position: number;
    duration: number;
    paused: boolean;
    track_window: {
      current_track: {
        name: string;
        artists: Array<{ name: string }>;
        album: { images: Array<{ url: string }> };
      };
    };
  }

  // ── Domain interfaces ───────────────────────────────────────────────────────
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
    track_id: string | null;
    track_name: string | null;
    artist_name: string | null;
    album_art_url: string | null;
    progress_ms: number | null;
    duration_ms: number | null;
  }

  interface Track {
    id: string;
    uri: string;
    title: string;
    artist: string;
    album: string;
    album_art_url: string | null;
    duration_ms: number;
  }

  interface Device {
    id: string | null;
    name: string;
    device_type: string;
    is_active: boolean;
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

  // Stop party confirmation (replaces window.confirm which Tauri blocks)
  let confirmingStop = $state(false);

  // Settings panel
  let showSettings = $state(false);
  let settingsPassword = $state('');
  let settingsRequestsPerGuest = $state(0);
  let settingsMaxQueueSize = $state(0);
  let settingsBlockExplicit = $state(false);
  let settingsSubdomain = $state('');
  let settingsAutoSkip = $state(false);
  let settingsAutoSkipMode = $state('percentage');
  let settingsAutoSkipThreshold = $state(50);
  let settingsSaving = $state(false);

  // SDK & device picker
  let sdkPlayer = $state<SpotifyPlayer | null>(null);
  let sdkDeviceId = $state<string | null>(null);
  let accessToken = $state<string | null>(null);
  let devices = $state<Device[]>([]);
  let showDevicePicker = $state(false);

  // Spotify's actual upcoming queue (separate from Spartify party queue)
  let spotifyQueue = $state<Track[]>([]);
  let filteredSpotifyQueue = $derived.by(() => {
    const partyIds = new Set(queue.map(e => e.track.id));
    return spotifyQueue.filter(t => !partyIds.has(t.id));
  });
  let controlLoading = $state(false);

  // Progress interpolation
  let syncedProgressMs = $state(0);
  let syncedAt = $state(0);
  let durationMs = $state(0);
  let isPlaying = $state(false);
  let displayProgressMs = $state(0);

  // Derived: current album art / track / artist from SDK or API fallback
  let nowPlayingArt = $derived(playback?.album_art_url ?? null);
  let nowPlayingTrack = $derived(playback?.track_name ?? null);
  let nowPlayingArtist = $derived(playback?.artist_name ?? null);

  let progressPct = $derived(durationMs > 0 ? (displayProgressMs / durationMs) * 100 : 0);

  // ── Intervals & WebSocket ─────────────────────────────────────────────────
  let pollInterval: ReturnType<typeof setInterval>;
  let progressInterval: ReturnType<typeof setInterval>;
  let ws: WebSocket | null = null;
  let wsReconnectTimeout: ReturnType<typeof setTimeout> | null = null;
  let wsDestroyed = false;

  // ── Lifecycle ──────────────────────────────────────────────────────────────
  onMount(async () => {
    invoke<string | null>('check_for_updates').then(v => { updateVersion = v; }).catch(() => {});
    invoke<{ join_password: string | null; requests_per_guest: number; max_queue_size: number; block_explicit: boolean; tunnel_subdomain: string | null; auto_skip_enabled: boolean; auto_skip_mode: string; auto_skip_threshold: number }>('get_party_settings').then(s => {
      settingsPassword = s.join_password ?? '';
      settingsRequestsPerGuest = s.requests_per_guest;
      settingsMaxQueueSize = s.max_queue_size;
      settingsBlockExplicit = s.block_explicit;
      settingsSubdomain = s.tunnel_subdomain ?? '';
      settingsAutoSkip = s.auto_skip_enabled;
      settingsAutoSkipMode = s.auto_skip_mode ?? 'percentage';
      settingsAutoSkipThreshold = s.auto_skip_threshold ?? 50;
    }).catch(() => {});

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
    await refreshDevices();

    // Get access token and init SDK
    try {
      const token = await invoke<string | null>('get_access_token');
      if (token) {
        accessToken = token;
        initSpotifySDK(token);
      }
    } catch (e) {
      console.error('Failed to get access token', e);
    }

    // 3s fallback poll for playback + Spotify queue
    pollInterval = setInterval(async () => {
      if (partyActive) {
        await refreshQueue();
        await refreshGuests();
      }
      await refreshPlayback();
      await refreshSpotifyQueue();
    }, 3000);

    // 500ms progress interpolation tick
    progressInterval = setInterval(() => {
      if (isPlaying) {
        displayProgressMs = Math.min(syncedProgressMs + (Date.now() - syncedAt), durationMs);
      } else {
        displayProgressMs = syncedProgressMs;
      }
    }, 500);
  });

  onDestroy(() => {
    clearInterval(pollInterval);
    clearInterval(progressInterval);
    disconnectWebSocket(true);
    if (sdkPlayer) {
      try { sdkPlayer.disconnect(); } catch {}
    }
  });

  // ── Spotify Web Playback SDK ───────────────────────────────────────────────
  function initSpotifySDK(token: string) {
    window.onSpotifyWebPlaybackSDKReady = () => {
      const player = new window.Spotify.Player({
        name: 'Spartify',
        getOAuthToken: (cb) => { cb(token); },
        volume: 0.8,
      });

      player.addListener('ready', async ({ device_id }) => {
        sdkDeviceId = device_id;
        try {
          await invoke('set_sdk_device_id', { deviceId: device_id });
        } catch (e) {
          console.error('Failed to set SDK device id', e);
        }
        await refreshDevices();
      });

      player.addListener('player_state_changed', (state) => {
        if (!state) return;
        const ct = state.track_window.current_track;
        // Update progress interpolation state
        syncedProgressMs = state.position;
        syncedAt = Date.now();
        durationMs = state.duration;
        isPlaying = !state.paused;
        displayProgressMs = state.position;

        // Mirror into playback so the sidebar reflects SDK data immediately
        playback = {
          is_playing: !state.paused,
          track_name: ct.name,
          artist_name: ct.artists.map(a => a.name).join(', '),
          album_art_url: ct.album.images[0]?.url ?? null,
          progress_ms: state.position,
          duration_ms: state.duration,
        };
      });

      player.addListener('not_ready', () => {
        sdkDeviceId = null;
      });

      player.connect();
      sdkPlayer = player;
    };

    // Inject SDK script if not already present
    if (!document.getElementById('spotify-sdk-script')) {
      const script = document.createElement('script');
      script.id = 'spotify-sdk-script';
      script.src = 'https://sdk.scdn.co/spotify-player.js';
      document.head.appendChild(script);
    }
  }

  // ── WebSocket ──────────────────────────────────────────────────────────────
  function connectWebSocket(url: string) {
    if (wsDestroyed) return;
    disconnectWebSocket(false);

    const wsUrl = url.replace('http://', 'ws://').replace('https://', 'wss://') + '/ws';
    ws = new WebSocket(wsUrl);

    ws.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.type === 'queue_update' && Array.isArray(msg.queue)) {
          queue = msg.queue;
        } else if (msg.type === 'guests_update' && Array.isArray(msg.guests)) {
          guests = msg.guests;
        }
      } catch {}
    };

    ws.onclose = () => {
      if (!wsDestroyed && partyActive) {
        wsReconnectTimeout = setTimeout(() => {
          if (!wsDestroyed && partyActive && localUrl) {
            connectWebSocket(localUrl);
          }
        }, 3000);
      }
    };

    ws.onerror = () => {
      ws?.close();
    };
  }

  function disconnectWebSocket(destroy: boolean) {
    if (destroy) wsDestroyed = true;
    if (wsReconnectTimeout) {
      clearTimeout(wsReconnectTimeout);
      wsReconnectTimeout = null;
    }
    if (ws) {
      ws.onclose = null;
      ws.close();
      ws = null;
    }
  }

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
      wsDestroyed = false;
      connectWebSocket(localUrl);
    } catch (e: unknown) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  async function stopParty() {
    if (!confirmingStop) { confirmingStop = true; return; }
    confirmingStop = false;
    await invoke('stop_party');
    partyActive = false;
    tunnelUrl = '';
    localUrl = '';
    qrDataUrl = '';
    queue = [];
    guests = [];
    spotifyQueue = [];
    disconnectWebSocket(false);
    wsDestroyed = false;
  }

  async function saveSettings() {
    settingsSaving = true;
    try {
      await invoke('save_party_settings', {
        settings: {
          join_password: settingsPassword.trim() || null,
          requests_per_guest: settingsRequestsPerGuest,
          max_queue_size: settingsMaxQueueSize,
          block_explicit: settingsBlockExplicit,
          tunnel_subdomain: settingsSubdomain.trim() || null,
          auto_skip_enabled: settingsAutoSkip,
          auto_skip_mode: settingsAutoSkipMode,
          auto_skip_threshold: settingsAutoSkipThreshold,
        }
      });
      showSettings = false;
    } catch (e: unknown) {
      error = String(e);
    } finally {
      settingsSaving = false;
    }
  }

  // ── Devices ────────────────────────────────────────────────────────────────
  async function refreshDevices() {
    try {
      devices = await invoke<Device[]>('get_devices');
    } catch {}
  }

  async function transferPlayback(deviceId: string) {
    try {
      await invoke('transfer_playback', { deviceId });
      await refreshDevices();
      showDevicePicker = false;
    } catch (e: unknown) {
      error = String(e);
    }
  }

  function toggleDevicePicker() {
    showDevicePicker = !showDevicePicker;
  }

  function handleDevicePickerOutsideClick(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (!target.closest('.device-picker-wrapper')) {
      showDevicePicker = false;
    }
  }

  let activeDeviceName = $derived.by(() => {
    const active = devices.find(d => d.is_active);
    if (active) return active.name;
    if (sdkDeviceId) return 'Spartify';
    return 'No device';
  });

  // ── Queue & playback ───────────────────────────────────────────────────────
  async function refreshQueue() {
    try { queue = await invoke<QueueEntry[]>('get_queue'); } catch {}
  }

  async function refreshGuests() {
    try { guests = await invoke<Guest[]>('get_guests'); } catch {}
  }

  async function kickGuest(id: string) {
    try { await invoke('kick_guest', { guestId: id }); } catch {}
  }

  async function banGuest(id: string, name: string) {
    if (!confirm(`Ban ${name}? They won't be able to rejoin from the same device.`)) return;
    try { await invoke('ban_guest', { guestId: id }); } catch {}
  }

  async function refreshPlayback() {
    try {
      const pb = await invoke<PlaybackState | null>('get_playback');
      if (pb) {
        // Only update progress from API if SDK isn't active
        if (!sdkPlayer) {
          if (pb.progress_ms != null) {
            syncedProgressMs = pb.progress_ms;
            syncedAt = Date.now();
          }
          if (pb.duration_ms != null) durationMs = pb.duration_ms;
          isPlaying = pb.is_playing;
        }
        playback = pb;
      }
    } catch {}
  }

  async function refreshSpotifyQueue() {
    try { spotifyQueue = await invoke<Track[]>('get_spotify_queue'); } catch {}
  }

  async function playbackControl(cmd: 'spotify_play' | 'spotify_pause' | 'spotify_skip_next' | 'spotify_skip_previous') {
    controlLoading = true;
    try {
      await invoke(cmd);
      // Give Spotify 500ms to update state, then refresh
      setTimeout(async () => {
        await refreshPlayback();
        await refreshSpotifyQueue();
        controlLoading = false;
      }, 500);
    } catch (e: unknown) {
      error = String(e);
      controlLoading = false;
    }
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

<svelte:window onclick={handleDevicePickerOutsideClick} />

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
      <div class="sidebar-top">
        <div class="logo">Spar<span>tify</span></div>
        <button class="gear-btn" onclick={() => { showSettings = !showSettings; confirmingStop = false; }} title="Party settings">⚙</button>
      </div>

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

        {#if confirmingStop}
          <div class="confirm-stop">
            <span>Stop the party?</span>
            <div class="confirm-btns">
              <button class="btn-confirm-yes" onclick={stopParty}>Stop</button>
              <button class="btn-confirm-no" onclick={() => confirmingStop = false}>Cancel</button>
            </div>
          </div>
        {:else}
          <button class="btn-stop" onclick={stopParty}>■ Stop Party</button>
        {/if}
      {/if}

      <!-- Now playing -->
      {#if playback}
        <div class="now-playing">
          <div class="np-label">Now Playing</div>
          <div class="np-body">
            {#if nowPlayingArt}
              <img class="np-art" src={nowPlayingArt} alt="" />
            {:else}
              <div class="np-art np-art-placeholder">♪</div>
            {/if}
            <div class="np-info">
              <div class="np-track">{nowPlayingTrack ?? '—'}</div>
              <div class="np-artist">{nowPlayingArtist ?? ''}</div>
              {#if isPlaying}
                <div class="playing-badge">▶ Playing</div>
              {:else}
                <div class="playing-badge paused">⏸ Paused</div>
              {/if}
            </div>
          </div>

          <!-- Progress bar -->
          <div class="progress-bar-wrapper">
            <div class="progress-bar">
              <div class="progress-fill" style="width: {progressPct}%"></div>
            </div>
            <div class="progress-times">
              <span>{fmtDuration(displayProgressMs)}</span>
              <span>{fmtDuration(durationMs)}</span>
            </div>
          </div>

          <!-- Playback controls -->
          <div class="pb-controls">
            <button class="pb-btn" onclick={() => playbackControl('spotify_skip_previous')} disabled={controlLoading} title="Previous">⏮</button>
            {#if isPlaying}
              <button class="pb-btn pb-btn-main" onclick={() => playbackControl('spotify_pause')} disabled={controlLoading} title="Pause">⏸</button>
            {:else}
              <button class="pb-btn pb-btn-main" onclick={() => playbackControl('spotify_play')} disabled={controlLoading} title="Play">▶</button>
            {/if}
            <button class="pb-btn" onclick={() => playbackControl('spotify_skip_next')} disabled={controlLoading} title="Skip">⏭</button>
          </div>
        </div>
      {:else}
        <div class="now-playing inactive">
          <div class="np-label">Now Playing</div>
          <p class="hint">Open Spotify on any device and start playing to enable queue control.</p>
        </div>
      {/if}

      <!-- Device picker -->
      <div class="device-picker-wrapper">
        <button class="device-btn" onclick={(e) => { e.stopPropagation(); toggleDevicePicker(); }}>
          <span class="device-icon">🔊</span>
          <span class="device-name">{activeDeviceName}</span>
          <span class="device-chevron">{showDevicePicker ? '▲' : '▼'}</span>
        </button>

        {#if showDevicePicker}
          <div class="device-dropdown" role="menu">
            <!-- Spartify (this device) option -->
            {#if sdkDeviceId}
              {@const isSdkActive = devices.find(d => d.id === sdkDeviceId)?.is_active ?? false}
              <button
                class="device-option"
                class:active={isSdkActive}
                onclick={(e) => { e.stopPropagation(); transferPlayback(sdkDeviceId!); }}
                role="menuitem"
              >
                <span class="device-option-name">Spartify (this device)</span>
                {#if isSdkActive}
                  <span class="device-active-dot"></span>
                {/if}
              </button>
            {/if}

            <!-- Other devices -->
            {#each devices.filter(d => d.id !== sdkDeviceId) as device (device.id ?? device.name)}
              <button
                class="device-option"
                class:active={device.is_active}
                onclick={(e) => { e.stopPropagation(); if (device.id) transferPlayback(device.id); }}
                role="menuitem"
                disabled={!device.id}
              >
                <span class="device-option-name">{device.name}</span>
                <span class="device-option-type">{device.device_type}</span>
                {#if device.is_active}
                  <span class="device-active-dot"></span>
                {/if}
              </button>
            {/each}

            {#if devices.length === 0 && !sdkDeviceId}
              <div class="device-empty">No devices found</div>
            {/if}
          </div>
        {/if}
      </div>

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
              <div class="guest-chip">
                <span class="guest-chip-name">{guest.name}</span>
                <button class="guest-action-btn" onclick={() => kickGuest(guest.id)} title="Kick">✕</button>
                <button class="guest-action-btn ban" onclick={() => banGuest(guest.id, guest.name)} title="Ban">⊘</button>
              </div>
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

      <div class="queue-list">
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
          {#each queue as entry, i (entry.track.id)}
            {#if i === 0}
              <div class="up-next-label">Party Queue</div>
            {/if}
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
        {/if}

        <!-- Spotify's actual upcoming queue -->
        {#if filteredSpotifyQueue.length > 0}
          <div class="spotify-queue-divider">
            <span>Spotify Autoplay</span>
          </div>
          {#each filteredSpotifyQueue.slice(0, 10) as track (track.id)}
            <div class="queue-item spotify-q-item">
              {#if track.album_art_url}
                <img class="art" src={track.album_art_url} alt="" />
              {:else}
                <div class="art placeholder">♪</div>
              {/if}
              <div class="track-info">
                <div class="title">{track.title}</div>
                <div class="sub">{track.artist} · {track.album}</div>
              </div>
              <div class="duration">{fmtDuration(track.duration_ms)}</div>
            </div>
          {/each}
        {/if}
      </div>
    </main>

    <!-- ── Settings panel ── -->
    {#if showSettings}
      <div class="settings-overlay" onclick={() => showSettings = false} role="presentation"></div>
      <div class="settings-panel">
        <div class="settings-header">
          <h2>Party Settings</h2>
          <button class="settings-close" onclick={() => showSettings = false}>✕</button>
        </div>

        <div class="settings-body">
          <div class="setting-group">
            <label class="setting-label" for="s-password">Join Password</label>
            <p class="setting-hint">Guests must enter this to join. Leave blank for no password.</p>
            <input
              id="s-password"
              type="text"
              class="setting-input"
              placeholder="No password"
              bind:value={settingsPassword}
              autocomplete="off"
            />
          </div>

          <div class="setting-group">
            <label class="setting-label" for="s-requests">Requests Per Guest</label>
            <p class="setting-hint">Max songs one guest can have in the queue at once. 0 = unlimited.</p>
            <input
              id="s-requests"
              type="number"
              class="setting-input"
              min="0"
              max="20"
              bind:value={settingsRequestsPerGuest}
            />
          </div>

          <div class="setting-group">
            <label class="setting-label" for="s-maxqueue">Max Queue Size</label>
            <p class="setting-hint">Total number of songs allowed in the queue. 0 = unlimited.</p>
            <input
              id="s-maxqueue"
              type="number"
              class="setting-input"
              min="0"
              max="200"
              bind:value={settingsMaxQueueSize}
            />
          </div>

          <div class="setting-group setting-toggle">
            <div>
              <div class="setting-label">Block Explicit Tracks</div>
              <p class="setting-hint">Reject songs marked explicit by Spotify.</p>
            </div>
            <button
              class="toggle-btn"
              class:on={settingsBlockExplicit}
              onclick={() => settingsBlockExplicit = !settingsBlockExplicit}
              role="switch"
              aria-checked={settingsBlockExplicit}
            >
              <span class="toggle-knob"></span>
            </button>
          </div>

          <div class="setting-group">
            <label class="setting-label" for="s-subdomain">Tunnel Subdomain</label>
            <p class="setting-hint">Your party URL prefix. Leave blank for a random subdomain.</p>
            <div class="subdomain-input-row">
              <input
                id="s-subdomain"
                type="text"
                class="setting-input"
                placeholder="random"
                bind:value={settingsSubdomain}
                autocomplete="off"
                spellcheck="false"
              />
              <span class="subdomain-suffix">.spartify.app</span>
            </div>
          </div>

          <div class="settings-divider"></div>

          <div class="setting-group setting-toggle">
            <div>
              <div class="setting-label">Auto-Skip Voting</div>
              <p class="setting-hint">Let guests vote on the current song. Skip it automatically when the threshold is hit.</p>
            </div>
            <button
              class="toggle-btn"
              class:on={settingsAutoSkip}
              onclick={() => settingsAutoSkip = !settingsAutoSkip}
              role="switch"
              aria-checked={settingsAutoSkip}
            >
              <span class="toggle-knob"></span>
            </button>
          </div>

          {#if settingsAutoSkip}
            <div class="setting-group">
              <label class="setting-label" for="s-skip-mode">Skip Trigger</label>
              <p class="setting-hint">How to measure when to skip.</p>
              <div class="skip-mode-row">
                <button
                  class="mode-btn"
                  class:active={settingsAutoSkipMode === 'percentage'}
                  onclick={() => settingsAutoSkipMode = 'percentage'}
                >% of guests</button>
                <button
                  class="mode-btn"
                  class:active={settingsAutoSkipMode === 'count'}
                  onclick={() => settingsAutoSkipMode = 'count'}
                ># of downvotes</button>
              </div>
            </div>

            <div class="setting-group">
              <label class="setting-label" for="s-skip-threshold">
                {settingsAutoSkipMode === 'percentage' ? 'Downvote Percentage (%)' : 'Downvote Count'}
              </label>
              <p class="setting-hint">
                {settingsAutoSkipMode === 'percentage'
                  ? 'Skip when this % of guests have downvoted the current song.'
                  : 'Skip when this many guests have downvoted the current song.'}
              </p>
              <input
                id="s-skip-threshold"
                type="number"
                class="setting-input"
                min={settingsAutoSkipMode === 'percentage' ? 1 : 1}
                max={settingsAutoSkipMode === 'percentage' ? 100 : 100}
                bind:value={settingsAutoSkipThreshold}
              />
            </div>
          {/if}
        </div>

        <div class="settings-footer">
          {#if error}
            <div class="error" style="margin-bottom:8px">{error}</div>
          {/if}
          <button class="btn-primary" onclick={saveSettings} disabled={settingsSaving}>
            {settingsSaving ? 'Saving…' : 'Save Settings'}
          </button>
        </div>
      </div>
    {/if}
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

  .sidebar-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-shrink: 0;
  }

  .logo {
    font-size: 1.6rem;
    font-weight: 800;
    letter-spacing: -0.5px;
    color: #fff;
  }
  .logo span { color: #1db954; }

  .gear-btn {
    background: none;
    border: none;
    color: #555;
    font-size: 1.1rem;
    cursor: pointer;
    padding: 4px 6px;
    border-radius: 6px;
    transition: color 0.15s, background 0.15s;
  }
  .gear-btn:hover { color: #ccc; background: #282828; }

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

  .np-label {
    font-size: 0.72rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 1px;
    color: #777;
  }

  .np-body { display: flex; gap: 12px; align-items: flex-start; }

  .np-art {
    width: 80px;
    height: 80px;
    border-radius: 6px;
    object-fit: cover;
    flex-shrink: 0;
  }

  .np-art-placeholder {
    width: 80px;
    height: 80px;
    border-radius: 6px;
    background: #333;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #555;
    font-size: 1.5rem;
    flex-shrink: 0;
  }

  .np-info { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }

  .np-track {
    font-size: 0.88rem;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .np-artist {
    font-size: 0.78rem;
    color: #b3b3b3;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .playing-badge {
    font-size: 0.72rem;
    color: #1db954;
    font-weight: 600;
    margin-top: 2px;
  }
  .playing-badge.paused { color: #777; }

  /* Progress bar */
  .progress-bar-wrapper {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .progress-bar {
    width: 100%;
    height: 3px;
    background: #444;
    border-radius: 2px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: #1db954;
    border-radius: 2px;
    transition: width 0.4s linear;
    max-width: 100%;
  }

  .progress-times {
    display: flex;
    justify-content: space-between;
    font-size: 0.7rem;
    color: #666;
  }

  /* Playback controls */
  .pb-controls {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding-top: 4px;
  }

  .pb-btn {
    background: #333;
    border: none;
    color: #ccc;
    border-radius: 50%;
    width: 34px;
    height: 34px;
    font-size: 0.85rem;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: background 0.15s, color 0.15s;
    flex-shrink: 0;
  }
  .pb-btn:hover:not(:disabled) { background: #444; color: #fff; }
  .pb-btn:disabled { opacity: 0.35; cursor: not-allowed; }

  .pb-btn-main {
    width: 40px;
    height: 40px;
    font-size: 1rem;
    background: #1db954;
    color: #000;
  }
  .pb-btn-main:hover:not(:disabled) { background: #17a349; color: #000; }

  /* Device picker */
  .device-picker-wrapper {
    position: relative;
  }

  .device-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    background: #282828;
    border: 1px solid #333;
    border-radius: 8px;
    padding: 8px 12px;
    color: #ccc;
    font-size: 0.82rem;
    cursor: pointer;
    transition: border-color 0.15s, background 0.15s;
    text-align: left;
  }
  .device-btn:hover { border-color: #555; background: #2f2f2f; }

  .device-icon { font-size: 0.9rem; flex-shrink: 0; }

  .device-name {
    flex: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    color: #ccc;
  }

  .device-chevron { font-size: 0.6rem; color: #555; flex-shrink: 0; }

  .device-dropdown {
    position: absolute;
    bottom: calc(100% + 6px);
    left: 0;
    right: 0;
    background: #2a2a2a;
    border: 1px solid #3a3a3a;
    border-radius: 8px;
    overflow: hidden;
    z-index: 100;
    box-shadow: 0 4px 20px rgba(0,0,0,0.5);
  }

  .device-option {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    background: none;
    border: none;
    padding: 10px 14px;
    color: #ccc;
    font-size: 0.82rem;
    cursor: pointer;
    text-align: left;
    transition: background 0.1s;
  }
  .device-option:hover:not(:disabled) { background: #333; color: #fff; }
  .device-option.active { color: #1db954; }
  .device-option:disabled { opacity: 0.4; cursor: not-allowed; }

  .device-option-name { flex: 1; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }

  .device-option-type {
    font-size: 0.72rem;
    color: #555;
    flex-shrink: 0;
    text-transform: capitalize;
  }

  .device-active-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: #1db954;
    flex-shrink: 0;
  }

  .device-empty {
    padding: 10px 14px;
    font-size: 0.8rem;
    color: #555;
    text-align: center;
  }

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
    padding: 4px 6px 4px 10px;
    font-size: 0.8rem;
    color: #ccc;
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }

  .guest-chip-name { line-height: 1.4; }

  .guest-action-btn {
    background: none;
    border: none;
    color: #666;
    cursor: pointer;
    font-size: 0.75rem;
    padding: 1px 3px;
    border-radius: 10px;
    line-height: 1;
    transition: color 0.15s, background 0.15s;
    flex-shrink: 0;
  }
  .guest-action-btn:hover { color: #ccc; background: #3a3a3a; }
  .guest-action-btn.ban:hover { color: #e74c3c; background: rgba(231,76,60,0.15); }

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

  /* "Up Next" label */
  .up-next-label {
    font-size: 0.72rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 1px;
    color: #1db954;
    padding: 4px 2px 2px;
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
  .queue-item.top {
    border-left: 3px solid #1db954;
    padding-left: 11px;
    padding-top: 13px;
    padding-bottom: 13px;
    background: #1f2a22;
  }
  .queue-item.top .title { font-size: 0.95rem; }
  .queue-item.top .art { width: 50px; height: 50px; }
  .queue-item.top .art.placeholder { width: 50px; height: 50px; }

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

  /* Spotify queue divider */
  .spotify-queue-divider {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 2px 6px;
    font-size: 0.72rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 1px;
    color: #555;
  }
  .spotify-queue-divider::before,
  .spotify-queue-divider::after {
    content: '';
    flex: 1;
    height: 1px;
    background: #282828;
  }

  .spotify-q-item {
    opacity: 0.6;
  }
  .spotify-q-item:hover { opacity: 0.85; }

  /* Stop confirmation */
  .confirm-stop {
    display: flex;
    flex-direction: column;
    gap: 8px;
    background: rgba(231,76,60,0.1);
    border: 1px solid rgba(231,76,60,0.3);
    border-radius: 8px;
    padding: 10px 12px;
  }
  .confirm-stop span { font-size: 0.85rem; color: #e74c3c; font-weight: 600; }
  .confirm-btns { display: flex; gap: 8px; }
  .btn-confirm-yes {
    flex: 1;
    background: #e74c3c;
    color: #fff;
    border: none;
    border-radius: 6px;
    padding: 7px;
    font-size: 0.85rem;
    font-weight: 700;
    cursor: pointer;
  }
  .btn-confirm-yes:hover { background: #c0392b; }
  .btn-confirm-no {
    flex: 1;
    background: #333;
    color: #ccc;
    border: none;
    border-radius: 6px;
    padding: 7px;
    font-size: 0.85rem;
    font-weight: 600;
    cursor: pointer;
  }
  .btn-confirm-no:hover { background: #3a3a3a; color: #fff; }

  /* Settings */
  .settings-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0,0,0,0.5);
    z-index: 50;
  }

  .settings-panel {
    position: fixed;
    top: 0;
    right: 0;
    bottom: 0;
    width: 340px;
    background: #1a1a1a;
    border-left: 1px solid #2a2a2a;
    z-index: 51;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .settings-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 20px 20px 16px;
    border-bottom: 1px solid #2a2a2a;
    flex-shrink: 0;
  }

  .settings-header h2 { font-size: 1.1rem; font-weight: 700; }

  .settings-close {
    background: none;
    border: none;
    color: #666;
    font-size: 1rem;
    cursor: pointer;
    padding: 4px 8px;
    border-radius: 4px;
  }
  .settings-close:hover { color: #fff; background: #333; }

  .settings-body {
    flex: 1;
    overflow-y: auto;
    padding: 16px 20px;
    display: flex;
    flex-direction: column;
    gap: 24px;
  }

  .setting-group { display: flex; flex-direction: column; gap: 6px; }
  .setting-group.setting-toggle { flex-direction: row; align-items: center; justify-content: space-between; gap: 16px; }
  .setting-group.setting-toggle > div { flex: 1; min-width: 0; }

  .setting-label {
    font-size: 0.85rem;
    font-weight: 600;
    color: #e0e0e0;
  }

  .setting-hint {
    font-size: 0.75rem;
    color: #666;
    line-height: 1.4;
  }

  .setting-input {
    background: #282828;
    border: 1px solid #333;
    border-radius: 6px;
    color: #fff;
    font-size: 0.9rem;
    padding: 9px 12px;
    outline: none;
    width: 100%;
    transition: border-color 0.15s;
    -moz-appearance: textfield;
  }
  .setting-input:focus { border-color: #1db954; }
  .setting-input::placeholder { color: #555; }
  .setting-input::-webkit-outer-spin-button,
  .setting-input::-webkit-inner-spin-button { -webkit-appearance: none; margin: 0; }

  .subdomain-input-row {
    display: flex;
    align-items: center;
    gap: 0;
  }
  .subdomain-input-row .setting-input {
    border-radius: 6px 0 0 6px;
    flex: 1;
  }
  .subdomain-suffix {
    background: #333;
    border: 1px solid #333;
    border-left: none;
    border-radius: 0 6px 6px 0;
    color: #888;
    font-size: 0.85rem;
    padding: 9px 10px;
    white-space: nowrap;
  }

  .settings-divider {
    height: 1px;
    background: #2a2a2a;
    margin: 4px 0;
  }

  .skip-mode-row {
    display: flex;
    gap: 8px;
    margin-top: 2px;
  }

  .mode-btn {
    flex: 1;
    padding: 8px 10px;
    background: #2a2a2a;
    border: 1px solid #333;
    border-radius: 6px;
    color: #888;
    font-size: 0.82rem;
    cursor: pointer;
    transition: border-color 0.15s, color 0.15s;
  }
  .mode-btn.active { border-color: #1db954; color: #1db954; background: rgba(29,185,84,0.08); }
  .mode-btn:hover:not(.active) { border-color: #555; color: #ccc; }

  /* Toggle switch */
  .toggle-btn {
    position: relative;
    width: 42px;
    height: 24px;
    background: #333;
    border: none;
    border-radius: 12px;
    cursor: pointer;
    flex-shrink: 0;
    transition: background 0.2s;
    padding: 0;
  }
  .toggle-btn.on { background: #1db954; }

  .toggle-knob {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 18px;
    height: 18px;
    background: #fff;
    border-radius: 50%;
    transition: transform 0.2s;
    pointer-events: none;
  }
  .toggle-btn.on .toggle-knob { transform: translateX(18px); }

  .settings-footer {
    padding: 16px 20px;
    border-top: 1px solid #2a2a2a;
    flex-shrink: 0;
  }
</style>
