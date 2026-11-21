<script lang="ts">
  import type { Snippet } from 'svelte';

  // 이름–값 목록. 안에는 KVRow만 둔다. 목록이 놓인 자리의 너비를 KVRow가 보고 좁은 배치로 바꾼다(컨테이너 이름 kv).
  let { children }: { children?: Snippet } = $props();
</script>

<dl>{@render children?.()}</dl>

<style>
  dl {
    container: kv / inline-size;
    display: flex;
    flex-direction: column;
    gap: var(--s2);
  }
</style>
