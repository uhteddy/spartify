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
<div style="display:flex;align-items:center;justify-content:center;height:100vh;background:#000;background-image:radial-gradient(ellipse 80% 50% at 50% 0%, rgba(29,185,84,0.1) 0%, transparent 60%);">
  <div style="text-align:center;font-family:'Outfit',-apple-system,sans-serif;">
    <div style="font-size:2.2rem;font-weight:800;color:#fff;margin-bottom:16px;letter-spacing:-1.5px;">
      Spar<span style="color:#1db954">tify</span>
    </div>
    <div style="width:20px;height:20px;border:2px solid rgba(255,255,255,0.1);border-top-color:#1db954;border-radius:50%;animation:spin 0.7s linear infinite;margin:0 auto;"></div>
    <style>@keyframes spin{to{transform:rotate(360deg)}}</style>
  </div>
</div>
