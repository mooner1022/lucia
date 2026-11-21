<script lang="ts">
  import type { Snippet } from 'svelte';

  // 이름–값 한 줄. 이름 곁에 값이 놓이고, 자리가 모자라면 값이 이름 아래로 내려간다.
  // 좁은 자리(폰)에서는 이름 칸과 값의 기준 너비를 줄여, 짧은 값이 이름 아래로 떨어지지 않게 한다.
  let { label, children }: { label: string; children?: Snippet } = $props();
</script>

<div>
  <dt>{label}</dt>
  <dd>{@render children?.()}</dd>
</div>

<style>
  div {
    display: flex;
    flex-wrap: wrap;
    gap: 0.1rem var(--s4);
  }
  dt {
    flex: 0 0 var(--label-w);
    max-width: 100%;
    color: var(--text-3);
    font-size: var(--fs-sm);
    line-height: 1.85;
  }
  dd {
    flex: 1 1 12rem;
    color: var(--text);
  }
  /* 이름 칸 7.5rem + 간격 + 값 12rem이면 360px 폰의 구역 안(약 294px)에 한 줄로 들어가지 않아 짧은 값도 이름 아래로 내려갔다.
     이름 6rem + 값 9rem이면 들어간다. 긴 값은 값 칸 안에서 줄을 바꾼다(전역 overflow-wrap: anywhere). --label-w는 그대로 둔다. */
  @container kv (max-width: 22rem) {
    div {
      column-gap: var(--s3);
    }
    dt {
      flex-basis: 6rem;
    }
    dd {
      flex-basis: 9rem;
    }
  }
</style>
