<script lang="ts">
  import type { Tone } from '../lib/types';

  // 상태 점(장식). 뜻은 곁의 글이 말한다.
  let { tone = 'muted' }: { tone?: Tone } = $props();
</script>

<span class="dot {tone}" aria-hidden="true"></span>

<style>
  .dot {
    display: inline-block;
    flex: none;
    width: 0.55rem;
    height: 0.55rem;
    border-radius: 50%;
    background: var(--muted);
    vertical-align: 0.05em;
  }
  .ok { background: var(--ok); }
  .info { background: var(--info); }
  .warn { background: var(--warn); }
  .bad { background: var(--bad); }
</style>
