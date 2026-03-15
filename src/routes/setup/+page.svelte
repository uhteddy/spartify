<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';

  let clientId = $state('');
  let step = $state(1); // 1 = instructions, 2 = paste ID, 3 = connecting
  let error = $state('');
  let loading = $state(false);
  let callbackUrl = $state('');
  let callbackError = $state('');

  const REDIRECT_URI = 'spartify://callback';

  onMount(async () => {
    try {
      const stored = await invoke<string | null>('get_stored_client_id');
      if (stored) clientId = stored;
    } catch {}
  });

  async function connect() {
    error = '';
    const id = clientId.trim();
    if (!id) { error = 'Please enter your Client ID.'; return; }
    if (id.length < 20) { error = 'That doesn\'t look like a valid Client ID.'; return; }

    loading = true;
    step = 3;
    try {
      await invoke('connect_spotify', { clientId: id });
      goto('/dashboard');
    } catch (e: unknown) {
      error = String(e);
      step = 2;
    } finally {
      loading = false;
    }
  }

  async function submitCallbackUrl() {
    callbackError = '';
    const url = callbackUrl.trim();
    if (!url) return;

    let code: string | null = null;
    try {
      // Accept either the full URL or just the bare code
      if (url.startsWith('spartify://') || url.startsWith('http')) {
        const parsed = new URL(url);
        code = parsed.searchParams.get('code');
      } else {
        code = url; // bare code pasted directly
      }
    } catch {
      callbackError = 'Could not parse that URL.';
      return;
    }

    if (!code) {
      callbackError = 'No "code" parameter found in that URL.';
      return;
    }

    try {
      await invoke('submit_oauth_code', { code });
    } catch (e: unknown) {
      callbackError = String(e);
    }
  }
</script>

<div class="page">
  <div class="card">
    <div class="logo">Spar<span>tify</span></div>
    <p class="tagline">Host Spotify parties with your friends</p>

    <!-- Step indicators -->
    <div class="steps">
      {#each [1, 2] as s}
        <div class="step" class:active={step >= s} class:current={step === s}>
          <div class="step-dot">{s}</div>
          <div class="step-label">{s === 1 ? 'Create App' : 'Connect'}</div>
        </div>
        {#if s < 2}
          <div class="step-line" class:active={step > s}></div>
        {/if}
      {/each}
    </div>

    {#if step === 1}
      <div class="step-content">
        <h2>Create a Spotify Developer App</h2>
        <p>
          Spartify uses the Spotify Web API, which requires your own free Developer App.
          This takes about 2 minutes and only needs to be done once.
        </p>

        <div class="instructions">
          <div class="instruction-step">
            <div class="num">1</div>
            <div>
              Go to <strong>developer.spotify.com/dashboard</strong> and log in with your Spotify account.
            </div>
          </div>
          <div class="instruction-step">
            <div class="num">2</div>
            <div>Click <strong>Create App</strong>. Give it any name (e.g. "My Spartify").</div>
          </div>
          <div class="instruction-step">
            <div class="num">3</div>
            <div>
              In the <strong>Redirect URIs</strong> field, add exactly:
              <div class="code-block">{REDIRECT_URI}</div>
              <span class="hint">Copy this precisely — it must match character-for-character.</span>
            </div>
          </div>
          <div class="instruction-step">
            <div class="num">4</div>
            <div>
              Set <strong>API/SDK</strong> to <em>Web API</em>. Accept the terms and save.
            </div>
          </div>
          <div class="instruction-step">
            <div class="num">5</div>
            <div>Copy your <strong>Client ID</strong> from the app settings page.</div>
          </div>
        </div>

        <div class="notice">
          <span class="notice-icon">⭐</span>
          <span>Spotify Premium is required to control playback.</span>
        </div>

        <button class="btn-primary" onclick={() => { step = 2; }}>
          I've created my app →
        </button>
      </div>

    {:else if step === 2}
      <div class="step-content">
        <h2>Paste your Client ID</h2>
        <p>Find your Client ID on the app's settings page at developer.spotify.com.</p>

        <div class="field">
          <label for="client-id">Spotify Client ID</label>
          <input
            id="client-id"
            type="text"
            placeholder="e.g. 1a2b3c4d5e6f..."
            bind:value={clientId}
            onkeydown={(e) => e.key === 'Enter' && connect()}
            autocomplete="off"
            spellcheck={false}
          />
        </div>

        {#if error}
          <div class="error">{error}</div>
        {/if}

        <button class="btn-primary" onclick={connect} disabled={loading}>
          {loading ? 'Connecting…' : 'Connect to Spotify'}
        </button>

        <button class="btn-ghost" onclick={() => { step = 1; error = ''; }}>
          ← Back
        </button>
      </div>

    {:else if step === 3}
      <div class="step-content centered">
        <div class="spinner-large"></div>
        <h2>Opening Spotify Login…</h2>
        <p>A browser window will open. Log in and authorize Spartify, then come back here.</p>
      </div>

      <div class="fallback-box">
        <p class="fallback-title">Browser didn't redirect automatically?</p>
        <p class="fallback-hint">
          After authorizing, your browser may show a "can't open" error for a
          <code>spartify://</code> URL. Copy that full URL from the address bar
          and paste it below.
        </p>
        <div class="field">
          <input
            type="text"
            placeholder="spartify://callback?code=..."
            bind:value={callbackUrl}
            onkeydown={(e) => e.key === 'Enter' && submitCallbackUrl()}
            autocomplete="off"
            spellcheck={false}
          />
        </div>
        {#if callbackError}
          <div class="error">{callbackError}</div>
        {/if}
        <button class="btn-primary" onclick={submitCallbackUrl} disabled={!callbackUrl.trim()}>
          Submit
        </button>
      </div>
    {/if}
  </div>
</div>

<style>
  .page {
    min-height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
    background: #121212;
    background-image: radial-gradient(ellipse 70% 40% at 50% 0%, rgba(29,185,84,0.05) 0%, transparent 100%);
  }

  .card {
    width: 100%;
    max-width: 520px;
    background: #181818;
    border-radius: 18px;
    padding: 36px;
    display: flex;
    flex-direction: column;
    gap: 22px;
    border: 1px solid rgba(255,255,255,0.07);
  }

  .logo {
    font-size: 1.9rem;
    font-weight: 800;
    letter-spacing: -1px;
    color: #ffffff;
    font-family: 'Outfit', sans-serif;
  }

  .logo span { color: #1db954; }

  .tagline {
    color: #b3b3b3;
    font-size: 0.88rem;
    margin-top: -14px;
  }

  /* Steps */
  .steps {
    display: flex;
    align-items: center;
    gap: 0;
    margin: 2px 0;
  }

  .step {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .step-dot {
    width: 26px;
    height: 26px;
    border-radius: 50%;
    background: #3e3e3e;
    color: #6a6a6a;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 0.78rem;
    font-weight: 700;
    transition: background 0.2s, color 0.2s;
  }

  .step.active .step-dot { background: #1db954; color: #000; }

  .step-label { font-size: 0.8rem; color: #b3b3b3; font-weight: 500; }
  .step.active .step-label { color: #ffffff; }

  .step-line {
    flex: 1;
    height: 1px;
    background: rgba(255,255,255,0.07);
    margin: 0 10px;
    transition: background 0.2s;
  }
  .step-line.active { background: #1db954; }

  /* Step content */
  .step-content {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .step-content.centered {
    align-items: center;
    text-align: center;
    padding: 20px 0;
  }

  h2 {
    font-size: 1.15rem;
    font-weight: 700;
    color: #ffffff;
    letter-spacing: -0.3px;
  }

  p { color: #b3b3b3; font-size: 0.88rem; line-height: 1.6; }

  /* Instructions */
  .instructions {
    display: flex;
    flex-direction: column;
    gap: 14px;
    background: #121212;
    border-radius: 10px;
    padding: 18px;
    border: 1px solid rgba(255,255,255,0.05);
  }

  .instruction-step {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    font-size: 0.875rem;
    color: #b3b3b3;
    line-height: 1.55;
  }

  .instruction-step .num {
    width: 22px;
    height: 22px;
    background: #2a2a2a;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 0.72rem;
    font-weight: 700;
    color: #1db954;
    flex-shrink: 0;
    margin-top: 1px;
  }

  .code-block {
    background: #2a2a2a;
    border-radius: 5px;
    padding: 4px 9px;
    font-family: 'Menlo', 'Consolas', monospace;
    font-size: 0.82rem;
    color: #1db954;
    display: inline-block;
    margin-top: 5px;
  }

  .hint { font-size: 0.78rem; color: #6a6a6a; margin-top: 2px; display: block; }

  /* Notice */
  .notice {
    display: flex;
    align-items: center;
    gap: 10px;
    background: rgba(29,185,84,0.06);
    border: 1px solid rgba(29,185,84,0.15);
    border-radius: 8px;
    padding: 10px 14px;
    font-size: 0.84rem;
    color: #b3b3b3;
  }

  /* Field */
  .field { display: flex; flex-direction: column; gap: 6px; }

  label { font-size: 0.84rem; font-weight: 600; color: #b3b3b3; }

  input[type="text"] {
    background: #2a2a2a;
    border: 1px solid rgba(255,255,255,0.08);
    border-radius: 8px;
    color: #ffffff;
    font-size: 0.95rem;
    padding: 12px 14px;
    outline: none;
    transition: border-color 0.15s;
    font-family: 'Menlo', 'Consolas', monospace;
    font-family: inherit;
  }
  input[type="text"]:focus { border-color: #1db954; }
  input::placeholder { color: #6a6a6a; }

  /* Buttons */
  .btn-primary {
    background: #1db954;
    color: #000;
    border: none;
    border-radius: 500px;
    padding: 13px 20px;
    font-size: 0.95rem;
    font-weight: 700;
    cursor: pointer;
    width: 100%;
    transition: background 0.15s, opacity 0.15s;
    font-family: 'Outfit', sans-serif;
    letter-spacing: -0.2px;
  }
  .btn-primary:hover { background: #179d47; }
  .btn-primary:disabled { opacity: 0.45; cursor: not-allowed; }

  .btn-ghost {
    background: none;
    border: none;
    color: #b3b3b3;
    font-size: 0.875rem;
    font-family: 'Outfit', sans-serif;
    cursor: pointer;
    padding: 4px 0;
    text-align: left;
    width: fit-content;
    transition: color 0.15s;
  }
  .btn-ghost:hover { color: #ffffff; }

  .error {
    background: rgba(224, 82, 82, 0.1);
    border: 1px solid rgba(224, 82, 82, 0.3);
    border-radius: 7px;
    padding: 10px 14px;
    font-size: 0.84rem;
    color: #e05252;
  }

  .fallback-box {
    background: #2a2a2a;
    border: 1px solid rgba(255,255,255,0.07);
    border-radius: 10px;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .fallback-title {
    font-size: 0.84rem;
    font-weight: 600;
    color: #b3b3b3;
    margin: 0;
  }

  .fallback-hint {
    font-size: 0.8rem;
    color: #b3b3b3;
    line-height: 1.55;
    margin: 0;
  }

  .fallback-hint code {
    color: #1db954;
    font-family: 'Menlo', 'Consolas', monospace;
    font-size: 0.85em;
  }

  .spinner-large {
    width: 44px;
    height: 44px;
    border: 3px solid rgba(255,255,255,0.08);
    border-top-color: #1db954;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    margin-bottom: 8px;
  }

  @keyframes spin { to { transform: rotate(360deg); } }
</style>
