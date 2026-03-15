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
    setVolume(volume: number): Promise<void>;
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
  let sdkVolume = $state(0.8);
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

  async function setVolume(value: number) {
    sdkVolume = value;
    if (sdkPlayer) {
      await sdkPlayer.setVolume(value);
    }
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

  let checkingUpdate = $state(false);
  let noUpdateToast = $state(false);

  async function checkForUpdates() {
    checkingUpdate = true;
    try {
      const v = await invoke<string | null>('check_for_updates');
      if (v) {
        updateVersion = v;
      } else {
        noUpdateToast = true;
        setTimeout(() => { noUpdateToast = false; }, 3000);
      }
    } catch (e) {
      console.error('Update check failed', e);
    } finally {
      checkingUpdate = false;
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
      <button class="update-dismiss" onclick={() => updateVersion = null} aria-label="Dismiss">
        <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
      </button>
    </div>
  {/if}

  <div class="layout">
    <!-- ── Left panel: share + guests ── -->
    <aside class="sidebar">
      <div class="sidebar-top">
        <div class="logo">Spar<span>tify</span></div>
        <div class="sidebar-actions">
          <button class="gear-btn" onclick={checkForUpdates} disabled={checkingUpdate} title={noUpdateToast ? 'Up to date!' : 'Check for updates'}>
            {#if checkingUpdate}
              <span class="spin-icon"></span>
            {:else if noUpdateToast}
              <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            {:else}
              <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="8 17 12 21 16 17"/><line x1="12" y1="12" x2="12" y2="21"/><path d="M20.88 18.09A5 5 0 0 0 18 9h-1.26A8 8 0 1 0 3 16.29"/></svg>
            {/if}
          </button>
          <button class="gear-btn" onclick={() => { showSettings = !showSettings; confirmingStop = false; }} title="Party settings">
            <svg viewBox="0 0 24 24" width="15" height="15" fill="currentColor"><path d="M19.14 12.94c.04-.3.06-.61.06-.94 0-.32-.02-.64-.07-.94l2.03-1.58c.18-.14.23-.41.12-.61l-1.92-3.32c-.12-.22-.37-.29-.59-.22l-2.39.96c-.5-.38-1.03-.7-1.62-.94l-.36-2.54c-.04-.24-.24-.41-.48-.41h-3.84c-.24 0-.43.17-.47.41l-.36 2.54c-.59.24-1.13.57-1.62.94l-2.39-.96c-.22-.08-.47 0-.59.22L2.74 8.87c-.12.21-.08.47.12.61l2.03 1.58c-.05.3-.09.63-.09.94s.02.64.07.94l-2.03 1.58c-.18.14-.23.41-.12.61l1.92 3.32c.12.22.37.29.59.22l2.39-.96c.5.38 1.03.7 1.62.94l.36 2.54c.05.24.24.41.48.41h3.84c.24 0 .44-.17.47-.41l.36-2.54c.59-.24 1.13-.57 1.62-.94l2.39.96c.22.08.47 0 .59-.22l1.92-3.32c.12-.22.07-.47-.12-.61l-2.01-1.58zM12 15.6c-1.98 0-3.6-1.62-3.6-3.6s1.62-3.6 3.6-3.6 3.6 1.62 3.6 3.6-1.62 3.6-3.6 3.6z"/></svg>
          </button>
        </div>
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
              Start Party
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
            <button class="icon-btn" onclick={copyUrl} title="Copy URL">
              <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
            </button>
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
          <button class="btn-stop" onclick={stopParty}>End Party</button>
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
                <div class="playing-badge">
                  <span class="eq-bars"><span class="eq-bar"></span><span class="eq-bar"></span><span class="eq-bar"></span></span>
                  Playing
                </div>
              {:else}
                <div class="playing-badge paused">Paused</div>
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
            <button class="pb-btn" onclick={() => playbackControl('spotify_skip_previous')} disabled={controlLoading} title="Previous">
              <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor"><path d="M6 6h2v12H6zm3.5 6 8.5 6V6z"/></svg>
            </button>
            {#if isPlaying}
              <button class="pb-btn pb-btn-main" onclick={() => playbackControl('spotify_pause')} disabled={controlLoading} title="Pause">
                <svg viewBox="0 0 24 24" width="22" height="22" fill="currentColor"><path d="M6 19h4V5H6v14zm8-14v14h4V5h-4z"/></svg>
              </button>
            {:else}
              <button class="pb-btn pb-btn-main" onclick={() => playbackControl('spotify_play')} disabled={controlLoading} title="Play">
                <svg viewBox="0 0 24 24" width="22" height="22" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>
              </button>
            {/if}
            <button class="pb-btn" onclick={() => playbackControl('spotify_skip_next')} disabled={controlLoading} title="Next">
              <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor"><path d="M6 18l8.5-6L6 6v12zm2.5-6 5.5 3.9V8.1L8.5 12zM16 6h2v12h-2z"/></svg>
            </button>
          </div>

          <!-- Volume control -->
          <div class="volume-control">
            <button class="pb-btn volume-icon" onclick={() => setVolume(sdkVolume === 0 ? 0.8 : 0)} title={sdkVolume === 0 ? 'Unmute' : 'Mute'}>
              {#if sdkVolume === 0}
                <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><path d="M16.5 12c0-1.77-1.02-3.29-2.5-4.03v2.21l2.45 2.45c.03-.2.05-.41.05-.63zm2.5 0c0 .94-.2 1.82-.54 2.64l1.51 1.51C20.63 14.91 21 13.5 21 12c0-4.28-2.99-7.86-7-8.77v2.06c2.89.86 5 3.54 5 6.71zM4.27 3 3 4.27 7.73 9H3v6h4l5 5v-6.73l4.25 4.25c-.67.52-1.42.93-2.25 1.18v2.06c1.38-.31 2.63-.95 3.69-1.81L19.73 21 21 19.73l-9-9L4.27 3zM12 4 9.91 6.09 12 8.18V4z"/></svg>
              {:else if sdkVolume < 0.5}
                <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><path d="M18.5 12c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM5 9v6h4l5 5V4L9 9H5z"/></svg>
              {:else}
                <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><path d="M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z"/></svg>
              {/if}
            </button>
            <input
              class="volume-slider"
              type="range"
              min="0"
              max="1"
              step="0.01"
              value={sdkVolume}
              disabled={!sdkPlayer}
              oninput={(e) => setVolume(parseFloat((e.target as HTMLInputElement).value))}
            />
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
          <span class="device-icon">
            <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor"><path d="M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02z"/></svg>
          </span>
          <span class="device-name">{activeDeviceName}</span>
          <span class="device-chevron">
            {#if showDevicePicker}
              <svg viewBox="0 0 24 24" width="10" height="10" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="18 15 12 9 6 15"/></svg>
            {:else}
              <svg viewBox="0 0 24 24" width="10" height="10" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>
            {/if}
          </span>
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
                <button class="guest-action-btn" onclick={() => kickGuest(guest.id)} title="Remove">
                  <svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
                </button>
                <button class="guest-action-btn ban" onclick={() => banGuest(guest.id, guest.name)} title="Ban">
                  <svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><line x1="4.93" y1="4.93" x2="19.07" y2="19.07"/></svg>
                </button>
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
        </div>
      </div>

      <div class="queue-list">
        {#if queue.length === 0}
          <div class="empty-queue">
            <div class="empty-icon">
              <svg viewBox="0 0 24 24" width="44" height="44" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>
            </div>
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
              >
                <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
              </button>
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
          <button class="settings-close" onclick={() => showSettings = false}>
            <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
          </button>
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
    background: rgba(29,185,84,0.08);
    border-bottom: 1px solid rgba(29,185,84,0.2);
    font-size: 0.84rem;
    color: #b3b3b3;
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
    font-family: 'Outfit', sans-serif;
  }
  .update-btn:disabled { opacity: 0.6; cursor: not-allowed; }

  .update-dismiss {
    background: none;
    border: none;
    color: #6a6a6a;
    cursor: pointer;
    padding: 5px;
    border-radius: 4px;
    flex-shrink: 0;
    transition: color 0.15s;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .update-dismiss:hover { color: #ffffff; }

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
    width: 288px;
    min-width: 288px;
    background: #181818;
    border-right: 1px solid rgba(255,255,255,0.06);
    display: flex;
    flex-direction: column;
    gap: 18px;
    padding: 22px 18px;
    overflow-y: auto;
  }

  .sidebar-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-shrink: 0;
  }

  .sidebar-actions {
    display: flex;
    align-items: center;
    gap: 2px;
  }

  .logo {
    font-size: 1.55rem;
    font-weight: 800;
    letter-spacing: -1px;
    color: #ffffff;
    font-family: 'Outfit', sans-serif;
  }
  .logo span { color: #1db954; }

  .gear-btn {
    background: none;
    border: none;
    color: #6a6a6a;
    cursor: pointer;
    padding: 6px;
    border-radius: 6px;
    transition: color 0.15s, background 0.15s;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
  }
  .gear-btn:hover { color: #b3b3b3; background: #2a2a2a; }
  .gear-btn:disabled { opacity: 0.5; cursor: not-allowed; }

  .hint {
    color: #6a6a6a;
    font-size: 0.82rem;
    line-height: 1.55;
  }

  .start-section { display: flex; flex-direction: column; gap: 12px; }

  .error {
    background: rgba(224,82,82,0.1);
    border: 1px solid rgba(224,82,82,0.25);
    border-radius: 7px;
    padding: 8px 12px;
    font-size: 0.82rem;
    color: #e05252;
  }

  /* Share card */
  .share-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    background: #282828;
    border-radius: 12px;
    padding: 16px;
    border: 1px solid rgba(255,255,255,0.05);
  }

  .qr {
    width: 156px;
    height: 156px;
    border-radius: 8px;
    display: block;
  }

  .url-row {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    background: #121212;
    border-radius: 7px;
    padding: 6px 10px;
    border: 1px solid rgba(255,255,255,0.05);
  }

  .url-text {
    flex: 1;
    font-size: 0.72rem;
    color: #1db954;
    font-family: 'Menlo', 'Consolas', monospace;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .icon-btn {
    background: none;
    border: none;
    color: #b3b3b3;
    cursor: pointer;
    padding: 4px;
    border-radius: 4px;
    flex-shrink: 0;
    transition: color 0.15s;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .icon-btn:hover { color: #ffffff; }

  /* Now playing — full-width album art */
  .now-playing {
    background: #282828;
    border-radius: 12px;
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    border: 1px solid rgba(255,255,255,0.05);
  }
  .now-playing.inactive { opacity: 0.5; }

  .np-label {
    font-size: 0.68rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 1.2px;
    color: #6a6a6a;
  }

  /* Stack album art on top of info (column layout) */
  .np-body { display: flex; flex-direction: column; gap: 10px; }

  .np-art {
    width: 100%;
    aspect-ratio: 1;
    border-radius: 8px;
    object-fit: cover;
    box-shadow: 0 8px 24px rgba(0,0,0,0.5);
    display: block;
  }

  .np-art-placeholder {
    width: 100%;
    aspect-ratio: 1;
    border-radius: 8px;
    background: #3e3e3e;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #6a6a6a;
    font-size: 2rem;
  }

  .np-info { display: flex; flex-direction: column; gap: 2px; min-width: 0; }

  .np-track {
    font-size: 0.92rem;
    font-weight: 700;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    letter-spacing: -0.3px;
    color: #ffffff;
  }
  .np-artist {
    font-size: 0.8rem;
    color: #b3b3b3;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .playing-badge {
    font-size: 0.68rem;
    color: #1db954;
    font-weight: 700;
    letter-spacing: 0.04em;
    margin-top: 3px;
    display: flex;
    align-items: center;
    gap: 5px;
  }
  .playing-badge.paused { color: #6a6a6a; }

  /* Animated equalizer bars */
  .eq-bars {
    display: inline-flex;
    align-items: flex-end;
    gap: 2px;
    height: 11px;
  }
  .eq-bar {
    width: 3px;
    background: #1db954;
    border-radius: 1px;
    animation: eq-bounce 0.9s ease-in-out infinite;
  }
  .eq-bar:nth-child(1) { height: 5px; animation-delay: 0s; }
  .eq-bar:nth-child(2) { height: 9px; animation-delay: 0.22s; }
  .eq-bar:nth-child(3) { height: 4px; animation-delay: 0.44s; }
  @keyframes eq-bounce {
    0%, 100% { transform: scaleY(0.45); }
    50% { transform: scaleY(1); }
  }

  /* Inline spin icon for gear button */
  .spin-icon {
    width: 13px;
    height: 13px;
    border: 2px solid rgba(255,255,255,0.15);
    border-top-color: currentColor;
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
    display: inline-block;
  }

  /* Progress bar */
  .progress-bar-wrapper {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }

  .progress-bar {
    width: 100%;
    height: 3px;
    background: #3e3e3e;
    border-radius: 3px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: #1db954;
    border-radius: 3px;
    transition: width 0.4s linear;
    max-width: 100%;
  }

  .progress-times {
    display: flex;
    justify-content: space-between;
    font-size: 0.68rem;
    color: #6a6a6a;
    font-variant-numeric: tabular-nums;
  }

  /* Playback controls */
  .pb-controls {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
  }

  .pb-btn {
    background: none;
    border: none;
    color: #b3b3b3;
    border-radius: 50%;
    width: 32px;
    height: 32px;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: color 0.15s, transform 0.1s;
    flex-shrink: 0;
    padding: 0;
  }
  .pb-btn:hover:not(:disabled) { color: #ffffff; transform: scale(1.08); }
  .pb-btn:disabled { opacity: 0.25; cursor: not-allowed; }

  .pb-btn-main {
    width: 44px;
    height: 44px;
    background: #ffffff;
    color: #000;
    border-radius: 50%;
    flex-shrink: 0;
    transition: background 0.15s, transform 0.1s;
  }
  .pb-btn-main:hover:not(:disabled) { background: #f0f0f0; transform: scale(1.05); }
  .pb-btn-main:disabled { opacity: 0.4; cursor: not-allowed; transform: none; }

  /* Volume control */
  .volume-control {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .volume-icon {
    flex-shrink: 0;
    color: #a0a0a0;
  }

  .volume-slider {
    -webkit-appearance: none;
    appearance: none;
    flex: 1;
    height: 3px;
    background: #3e3e3e;
    border-radius: 3px;
    outline: none;
    cursor: pointer;
  }

  .volume-slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: #ffffff;
    cursor: pointer;
    transition: transform 0.1s;
  }

  .volume-slider::-webkit-slider-thumb:hover {
    transform: scale(1.2);
    background: #1db954;
  }

  .volume-slider:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  /* Device picker */
  .device-picker-wrapper {
    position: relative;
  }

  .device-btn {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    background: #282828;
    border: 1px solid rgba(255,255,255,0.07);
    border-radius: 8px;
    padding: 8px 12px;
    color: #b3b3b3;
    font-size: 0.82rem;
    font-family: 'Outfit', sans-serif;
    cursor: pointer;
    transition: border-color 0.15s;
    text-align: left;
  }
  .device-btn:hover { border-color: rgba(255,255,255,0.15); color: #b3b3b3; }

  .device-icon { display: flex; align-items: center; flex-shrink: 0; }

  .device-name {
    flex: 1;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .device-chevron { display: flex; align-items: center; color: #6a6a6a; flex-shrink: 0; }

  .device-dropdown {
    position: absolute;
    bottom: calc(100% + 6px);
    left: 0;
    right: 0;
    background: #2a2a2a;
    border: 1px solid rgba(255,255,255,0.09);
    border-radius: 10px;
    overflow: hidden;
    z-index: 100;
    box-shadow: 0 8px 24px rgba(0,0,0,0.6);
  }

  .device-option {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    background: none;
    border: none;
    padding: 10px 14px;
    color: #b3b3b3;
    font-size: 0.82rem;
    font-family: 'Outfit', sans-serif;
    cursor: pointer;
    text-align: left;
    transition: background 0.1s, color 0.1s;
  }
  .device-option:hover:not(:disabled) { background: #3e3e3e; color: #ffffff; }
  .device-option.active { color: #1db954; }
  .device-option:disabled { opacity: 0.4; cursor: not-allowed; }

  .device-option-name { flex: 1; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }

  .device-option-type {
    font-size: 0.7rem;
    color: #6a6a6a;
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
    color: #6a6a6a;
    text-align: center;
  }

  /* Guests */
  .guests-section { display: flex; flex-direction: column; gap: 8px; }

  .section-header {
    font-size: 0.68rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 1.2px;
    color: #6a6a6a;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .badge {
    background: #3e3e3e;
    color: #b3b3b3;
    border-radius: 10px;
    padding: 1px 7px;
    font-size: 0.7rem;
  }

  .guest-list { display: flex; flex-wrap: wrap; gap: 5px; }

  .guest-chip {
    background: #2a2a2a;
    border: 1px solid rgba(255,255,255,0.06);
    border-radius: 20px;
    padding: 4px 6px 4px 10px;
    font-size: 0.78rem;
    color: #b3b3b3;
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }

  .guest-chip-name { line-height: 1.4; }

  .guest-action-btn {
    background: none;
    border: none;
    color: #6a6a6a;
    cursor: pointer;
    padding: 3px;
    border-radius: 6px;
    line-height: 1;
    transition: color 0.15s, background 0.15s;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .guest-action-btn:hover { color: #b3b3b3; background: #3e3e3e; }
  .guest-action-btn.ban:hover { color: #e05252; background: rgba(224,82,82,0.12); }

  /* Buttons */
  .btn-primary {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    background: #1db954;
    color: #000;
    border: none;
    border-radius: 500px;
    padding: 10px 16px;
    font-size: 0.9rem;
    font-weight: 700;
    font-family: 'Outfit', sans-serif;
    cursor: pointer;
    transition: background 0.15s, opacity 0.15s;
    letter-spacing: -0.2px;
  }
  .btn-primary.big { padding: 13px 16px; font-size: 0.95rem; }
  .btn-primary:hover:not(:disabled) { background: #179d47; }
  .btn-primary:disabled { opacity: 0.4; cursor: not-allowed; }

  .btn-stop {
    background: transparent;
    border: 1px solid rgba(255,255,255,0.1);
    color: #b3b3b3;
    border-radius: 8px;
    padding: 8px 14px;
    font-size: 0.84rem;
    font-weight: 600;
    font-family: 'Outfit', sans-serif;
    cursor: pointer;
    transition: border-color 0.15s, color 0.15s;
  }
  .btn-stop:hover { border-color: rgba(224,82,82,0.5); color: #e05252; }

  .spinner {
    width: 15px;
    height: 15px;
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
    padding: 22px 26px 14px;
    border-bottom: 1px solid rgba(255,255,255,0.06);
    gap: 12px;
    flex-shrink: 0;
  }

  .queue-header h1 {
    font-size: 1.35rem;
    font-weight: 800;
    color: #ffffff;
    letter-spacing: -0.5px;
    font-family: 'Outfit', sans-serif;
  }

  .track-count {
    font-size: 0.78rem;
    color: #6a6a6a;
    margin-left: 8px;
    font-variant-numeric: tabular-nums;
  }

  .queue-actions {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .error-inline { font-size: 0.82rem; color: #e05252; max-width: 200px; }

  /* Empty state */
  .empty-queue {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: #6a6a6a;
    font-size: 0.88rem;
    text-align: center;
    padding: 40px;
  }

  .empty-icon { margin-bottom: 10px; opacity: 0.45; color: #6a6a6a; display: flex; align-items: center; justify-content: center; }

  /* Queue list */
  .queue-list {
    flex: 1;
    overflow-y: auto;
    padding: 10px 18px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  /* "Up Next" label */
  .up-next-label {
    font-size: 0.68rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 1.2px;
    color: #1db954;
    padding: 4px 4px 2px;
  }

  .queue-item {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 9px 12px;
    border-radius: 9px;
    transition: background 0.12s;
  }
  .queue-item:hover { background: #282828; }
  .queue-item.top {
    border-left: 2px solid #1db954;
    padding-left: 10px;
    padding-top: 11px;
    padding-bottom: 11px;
    background: rgba(29,185,84,0.05);
  }
  .queue-item.top:hover { background: rgba(29,185,84,0.08); }
  .queue-item.top .title { font-size: 0.95rem; }
  .queue-item.top .art { width: 48px; height: 48px; }
  .queue-item.top .art.placeholder { width: 48px; height: 48px; }

  .pos {
    width: 22px;
    text-align: center;
    font-size: 0.78rem;
    color: #6a6a6a;
    font-weight: 700;
    flex-shrink: 0;
    font-variant-numeric: tabular-nums;
  }

  .art {
    width: 42px;
    height: 42px;
    border-radius: 5px;
    object-fit: cover;
    flex-shrink: 0;
    box-shadow: 0 2px 8px rgba(0,0,0,0.3);
  }

  .art.placeholder {
    background: #2a2a2a;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #6a6a6a;
    font-size: 1rem;
  }

  .track-info { flex: 1; min-width: 0; }

  .title {
    font-size: 0.9rem;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    letter-spacing: -0.2px;
    color: #ffffff;
  }

  .sub {
    font-size: 0.77rem;
    color: #b3b3b3;
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
    font-size: 0.76rem;
    font-weight: 600;
    color: #6a6a6a;
    font-variant-numeric: tabular-nums;
  }
  .votes.positive { color: #1db954; }
  .votes.negative { color: #e05252; }

  .duration {
    font-size: 0.72rem;
    color: #6a6a6a;
    font-variant-numeric: tabular-nums;
  }

  .remove-btn {
    background: none;
    border: none;
    color: #3e3e3e;
    cursor: pointer;
    padding: 5px;
    border-radius: 5px;
    flex-shrink: 0;
    transition: color 0.15s, background 0.15s;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .remove-btn:hover { color: #e05252; background: rgba(224,82,82,0.08); }

  /* Spotify queue divider */
  .spotify-queue-divider {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 4px 6px;
    font-size: 0.68rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 1.2px;
    color: #6a6a6a;
  }
  .spotify-queue-divider::before,
  .spotify-queue-divider::after {
    content: '';
    flex: 1;
    height: 1px;
    background: rgba(255,255,255,0.06);
  }

  .spotify-q-item { opacity: 0.55; }
  .spotify-q-item:hover { opacity: 0.8; }

  /* Stop confirmation */
  .confirm-stop {
    display: flex;
    flex-direction: column;
    gap: 8px;
    background: rgba(224,82,82,0.08);
    border: 1px solid rgba(224,82,82,0.25);
    border-radius: 9px;
    padding: 10px 12px;
  }
  .confirm-stop span { font-size: 0.84rem; color: #e05252; font-weight: 600; }
  .confirm-btns { display: flex; gap: 8px; }
  .btn-confirm-yes {
    flex: 1;
    background: #e05252;
    color: #fff;
    border: none;
    border-radius: 6px;
    padding: 7px;
    font-size: 0.84rem;
    font-weight: 700;
    font-family: 'Outfit', sans-serif;
    cursor: pointer;
    transition: background 0.15s;
  }
  .btn-confirm-yes:hover { background: #c94040; }
  .btn-confirm-no {
    flex: 1;
    background: #3e3e3e;
    color: #b3b3b3;
    border: none;
    border-radius: 6px;
    padding: 7px;
    font-size: 0.84rem;
    font-weight: 600;
    font-family: 'Outfit', sans-serif;
    cursor: pointer;
    transition: background 0.15s, color 0.15s;
  }
  .btn-confirm-no:hover { background: #4a4a4a; color: #ffffff; }

  /* Settings */
  .settings-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0,0,0,0.55);
    z-index: 50;
    backdrop-filter: blur(2px);
  }

  .settings-panel {
    position: fixed;
    top: 0;
    right: 0;
    bottom: 0;
    width: 340px;
    background: #181818;
    border-left: 1px solid rgba(255,255,255,0.07);
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
    border-bottom: 1px solid rgba(255,255,255,0.07);
    flex-shrink: 0;
  }

  .settings-header h2 {
    font-size: 1.05rem;
    font-weight: 700;
    letter-spacing: -0.3px;
    color: #ffffff;
  }

  .settings-close {
    background: none;
    border: none;
    color: #6a6a6a;
    cursor: pointer;
    padding: 6px;
    border-radius: 5px;
    transition: color 0.15s, background 0.15s;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .settings-close:hover { color: #ffffff; background: #3e3e3e; }

  .settings-body {
    flex: 1;
    overflow-y: auto;
    padding: 16px 20px;
    display: flex;
    flex-direction: column;
    gap: 22px;
  }

  .setting-group { display: flex; flex-direction: column; gap: 6px; }
  .setting-group.setting-toggle { flex-direction: row; align-items: center; justify-content: space-between; gap: 16px; }
  .setting-group.setting-toggle > div { flex: 1; min-width: 0; }

  .setting-label {
    font-size: 0.84rem;
    font-weight: 600;
    color: #b3b3b3;
  }

  .setting-hint {
    font-size: 0.74rem;
    color: #6a6a6a;
    line-height: 1.45;
  }

  .setting-input {
    background: #282828;
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 7px;
    color: #ffffff;
    font-size: 0.88rem;
    font-family: 'Outfit', sans-serif;
    padding: 9px 12px;
    outline: none;
    width: 100%;
    transition: border-color 0.15s;
    -moz-appearance: textfield;
  }
  .setting-input:focus { border-color: #1db954; }
  .setting-input::placeholder { color: #6a6a6a; }
  .setting-input::-webkit-outer-spin-button,
  .setting-input::-webkit-inner-spin-button { -webkit-appearance: none; margin: 0; }

  .subdomain-input-row {
    display: flex;
    align-items: center;
    gap: 0;
  }
  .subdomain-input-row .setting-input {
    border-radius: 7px 0 0 7px;
    flex: 1;
  }
  .subdomain-suffix {
    background: #2a2a2a;
    border: 1px solid rgba(255,255,255,0.08);
    border-left: none;
    border-radius: 0 7px 7px 0;
    color: #6a6a6a;
    font-size: 0.82rem;
    padding: 9px 10px;
    white-space: nowrap;
  }

  .settings-divider {
    height: 1px;
    background: rgba(255,255,255,0.06);
    margin: 2px 0;
  }

  .skip-mode-row {
    display: flex;
    gap: 8px;
    margin-top: 2px;
  }

  .mode-btn {
    flex: 1;
    padding: 8px 10px;
    background: #282828;
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 7px;
    color: #6a6a6a;
    font-size: 0.8rem;
    font-family: 'Outfit', sans-serif;
    cursor: pointer;
    transition: border-color 0.15s, color 0.15s;
  }
  .mode-btn.active { border-color: #1db954; color: #1db954; background: rgba(29,185,84,0.06); }
  .mode-btn:hover:not(.active) { border-color: rgba(255,255,255,0.15); color: #b3b3b3; }

  /* Toggle switch */
  .toggle-btn {
    position: relative;
    width: 40px;
    height: 23px;
    background: #3e3e3e;
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
    width: 17px;
    height: 17px;
    background: #fff;
    border-radius: 50%;
    transition: transform 0.2s;
    pointer-events: none;
    box-shadow: 0 1px 3px rgba(0,0,0,0.3);
  }
  .toggle-btn.on .toggle-knob { transform: translateX(17px); }

  .settings-footer {
    padding: 16px 20px;
    border-top: 1px solid rgba(255,255,255,0.07);
    flex-shrink: 0;
  }
</style>
