<script lang="ts">
  import Dot from './Dot.svelte';
  import Time from './Time.svelte';

  // "지금 실행" 동작의 마지막 결과 한 줄.
  let { run }: { run?: { at: string; ok: boolean; text: string } | null } = $props();
</script>

<p>
  {#if run}
    <Dot tone={run.ok ? 'ok' : 'bad'} />
    <span>{run.ok ? '마지막 성공 ' : '마지막 실패 '}<Time at={run.at} /> · {run.text}</span>
  {:else}
    <Dot tone="muted" /><span>아직 실행 기록이 없습니다</span>
  {/if}
</p>

<style>
  p {
    display: flex;
    align-items: baseline;
    gap: var(--s2);
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
</style>
