<script lang="ts">
  import { getContext, type Snippet } from 'svelte';

  // 표의 한 칸. 이름은 Table의 head에서 순서대로 온다(조건에 따라 빠지는 칸이 있으면 `label`을 직접 준다).
  // `main`은 줄의 대표 칸(좁을 때 굵게, 이름 없이). `num`은 숫자·시각 칸(넓을 때 오른쪽 맞춤, 줄을 바꾸지 않는다).
  let {
    label,
    main = false,
    num = false,
    children,
  }: { label?: string; main?: boolean; num?: boolean; children?: Snippet } = $props();
  const head = getContext<(() => string[]) | undefined>('table-head');
  const index = getContext<(() => number) | undefined>('table-row')?.() ?? 0;
  const name = $derived(label ?? head?.()[index] ?? '');
</script>

<td data-label={name} class:main class:num><div>{@render children?.()}</div></td>

<style>
  td {
    padding: var(--s3);
    vertical-align: top;
  }
  td.num {
    text-align: end;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  /* 좁을 때: 이름과 값이 한 줄에 나란하다. 카드가 좁아도 짧은 값이 이름 곁에 남도록 이름 칸은 6rem, 값의 기준 너비는 9rem이다
     (--label-w 7.5rem과 12rem이면 294px 폭의 카드 안(안쪽 약 260px)에서 값이 이름 아래로 떨어진다). 긴 값은 값 칸 안에서 줄을 바꾼다.
     문턱은 Table의 칸 수에 따라 달라서(컨테이너 이름 table-few·table-many) 규칙이 둘이다. */
  @container table-few (max-width: 18rem) {
    td {
      display: flex;
      flex-wrap: wrap;
      gap: 0 var(--s3);
      padding: 0;
    }
    td::before {
      content: attr(data-label);
      flex: 0 0 6rem;
      max-width: 100%;
      color: var(--text-3);
    }
    td.main {
      font-weight: 600;
      font-size: var(--fs-body);
    }
    td > div {
      flex: 1 1 9rem;
    }
    td.num {
      text-align: start;
      white-space: normal;
    }
    td.main::before,
    td[data-label='']::before {
      content: none;
    }
  }
  @container table-many (max-width: 30rem) {
    td {
      display: flex;
      flex-wrap: wrap;
      gap: 0 var(--s3);
      padding: 0;
    }
    td::before {
      content: attr(data-label);
      flex: 0 0 6rem;
      max-width: 100%;
      color: var(--text-3);
    }
    td.main {
      font-weight: 600;
      font-size: var(--fs-body);
    }
    td > div {
      flex: 1 1 9rem;
    }
    td.num {
      text-align: start;
      white-space: normal;
    }
    td.main::before,
    td[data-label='']::before {
      content: none;
    }
  }
</style>
