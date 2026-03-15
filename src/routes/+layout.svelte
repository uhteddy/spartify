<script lang="ts">
  let { children } = $props();
</script>

{@render children()}

<style>
  :global(*) { box-sizing: border-box; margin: 0; padding: 0; }

  :global(body) {
    font-family: 'Outfit', -apple-system, sans-serif;
    background: #121212;
    color: #ffffff;
    overflow: hidden;
  }

  :global(::-webkit-scrollbar) { width: 5px; }
  :global(::-webkit-scrollbar-track) { background: transparent; }
  :global(::-webkit-scrollbar-thumb) { background: #2c2c2c; border-radius: 3px; }
  :global(::-webkit-scrollbar-thumb:hover) { background: #3a3a3a; }
</style>
