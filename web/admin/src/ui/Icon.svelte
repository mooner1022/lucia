<script lang="ts">
  import { ICONS, ICON_FALLBACK } from './icons';

  let { name, size = 'md' }: { name: string; size?: 'sm' | 'md' | 'lg' | 'xl' } = $props();
</script>

<svg
  class={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="2"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  focusable="false">{@html ICONS[name] ?? ICON_FALLBACK}</svg
>

<style>
  svg {
    flex: none;
    width: 1.15em;
    height: 1.15em;
    vertical-align: -0.2em;
  }
  .sm {
    width: 1em;
    height: 1em;
  }
  .lg {
    width: 1.375rem;
    height: 1.375rem;
  }
  .xl {
    width: 1.75rem;
    height: 1.75rem;
  }
</style>
