<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';

  onMount(async () => {
    try {
      const status = await invoke<{ connected: boolean; client_id: string | null }>('get_spotify_status');
      if (status.connected) {
        goto('/dashboard');
      } else {
        goto('/setup');
      }
    } catch {
      goto('/setup');
    }
  });
</script>

<!-- Blank splash while we check auth status -->
<div style="display:flex;align-items:center;justify-content:center;height:100vh;background:#121212;">
  <div style="text-align:center;color:#b3b3b3;font-family:sans-serif;">
    <div style="font-size:2rem;font-weight:800;color:#fff;margin-bottom:12px;">
      Spar<span style="color:#1db954">tify</span>
    </div>
    <div>Loading…</div>
  </div>
</div>
