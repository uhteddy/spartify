<script lang="ts">
  let { children } = $props();
</script>

{@render children()}

<style>
  :global(*) { box-sizing: border-box; margin: 0; padding: 0; }

  :global(body) {
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
    background: #121212;
    color: #fff;
    overflow: hidden;
  }

  :global(::-webkit-scrollbar) { width: 6px; }
  :global(::-webkit-scrollbar-track) { background: transparent; }
  :global(::-webkit-scrollbar-thumb) { background: #333; border-radius: 3px; }
  :global(::-webkit-scrollbar-thumb:hover) { background: #555; }
</style>
