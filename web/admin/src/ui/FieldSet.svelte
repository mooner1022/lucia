<script lang="ts">
  import type { Snippet } from 'svelte';

  // 한 묶음의 고르기(라디오, 체크 상자 여럿). 제목이 묶음의 이름이 된다.
  let { legend, help, children }: { legend: string; help?: string; children?: Snippet } = $props();
</script>

<fieldset>
  <legend>{legend}</legend>
  {#if help}<p>{help}</p>{/if}
  <div>{@render children?.()}</div>
</fieldset>

<style>
  fieldset {
    display: flex;
    flex-direction: column;
    gap: var(--s2);
  }
  legend {
    padding: 0;
    margin-bottom: var(--s1);
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 650;
  }
  p {
    font-size: var(--fs-sm);
    color: var(--text-3);
  }
  div {
    display: flex;
    flex-direction: column;
    gap: var(--s2);
  }
</style>
