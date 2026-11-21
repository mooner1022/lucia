<script lang="ts">
  import { setContext, type Snippet } from 'svelte';

  let { children }: { children?: Snippet } = $props();
  let next = 0;
  setContext('table-row', () => next++);
</script>

<tr>{@render children?.()}</tr>

<style>
  /* 넓을 때: 줄 사이는 옅은 실선(칸 위에 긋는다. 둥근 표에서는 줄 테두리가 그려지지 않는다), 가리킨 줄은 번짐 색. */
  tr + :global(tr) > :global(td) {
    border-top: 1px solid var(--border);
  }
  tr:hover {
    background: color-mix(in srgb, var(--accent) 5%, transparent);
  }
  /* 좁을 때: 줄마다 작은 카드. 문턱은 Table의 칸 수에 따라 달라서(컨테이너 이름 table-few·table-many) 규칙이 둘이다. */
  @container table-few (max-width: 18rem) {
    tr + :global(tr) > :global(td) {
      border-top: 0;
    }
    tr {
      display: flex;
      flex-direction: column;
      gap: var(--s1);
      padding: var(--s3) var(--s4);
      border: 1px solid var(--border);
      border-radius: var(--r-md);
      background: var(--surface);
    }
    tr + :global(tr) {
      margin-top: var(--s2);
    }
  }
  @container table-many (max-width: 30rem) {
    tr + :global(tr) > :global(td) {
      border-top: 0;
    }
    tr {
      display: flex;
      flex-direction: column;
      gap: var(--s1);
      padding: var(--s3) var(--s4);
      border: 1px solid var(--border);
      border-radius: var(--r-md);
      background: var(--surface);
    }
    tr + :global(tr) {
      margin-top: var(--s2);
    }
  }
</style>
