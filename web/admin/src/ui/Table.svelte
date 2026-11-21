<script lang="ts">
  import { setContext, type Snippet } from 'svelte';

  // 표. 넓으면 표로, 좁으면(표가 놓인 자리 기준) 줄마다 "이름: 값"으로 쌓인 카드로 바뀐다.
  // head에 칸 이름을 주고, 안에는 Row와 Cell을 둔다. Cell의 이름은 순서대로 head에서 온다.
  // `num`에는 숫자·시각 칸의 순번(0부터)을 준다: 머리 칸도 오른쪽으로 맞춘다(그 칸의 Cell에는 `num`을 준다).
  // 카드로 바뀌는 너비는 칸 수로 정한다: 세 칸 이하는 아주 좁을 때(18rem 아래)만, 그보다 많으면 30rem 아래에서.
  // 칸이 적은 표는 두 단 배치의 카드 안(약 26rem)에서도 한 줄에 다 들어가서, 카드로 쌓으면 이름만 되풀이된다.
  let {
    head,
    caption,
    num = [],
    children,
  }: { head: string[]; caption?: string; num?: number[]; children?: Snippet } = $props();
  setContext('table-head', () => head);
  const few = $derived(head.length <= 3);
</script>

<!-- 컨테이너 이름이 칸 수에 따라 달라서 Table·Row·Cell이 각자 맞는 문턱의 @container를 쓴다. -->
<div class="wrap" class:few class:many={!few}>
  <table>
    {#if caption}<caption class="sr-only">{caption}</caption>{/if}
    <thead>
      <tr>
        {#each head as name, i (name)}<th scope="col" class:num={num.includes(i)}>{name}</th>{/each}
      </tr>
    </thead>
    <tbody>{@render children?.()}</tbody>
  </table>
</div>

<style>
  .wrap {
    container-type: inline-size;
  }
  .few {
    container-name: table-few;
  }
  .many {
    container-name: table-many;
  }
  /* 넓을 때: 머리 줄은 옅은 면(바깥 모서리가 둥글다), 줄 사이는 옅은 실선, 가리킨 줄은 번짐 색(Row).
     머리 줄과 번짐이 글 곁으로 한 칸(--s3) 더 나오도록 표를 양옆으로 그만큼 넓혀 글의 자리는 그대로 둔다.
     (구역 카드의 안쪽 여백이 이보다 넓다.) 둥근 모서리는 border-collapse: separate에서만 먹는다. */
  table {
    width: calc(100% + var(--s3) * 2);
    margin-inline: calc(var(--s3) * -1);
    border-collapse: separate;
    border-spacing: 0;
    font-size: var(--fs-sm);
  }
  th {
    padding: var(--s2) var(--s3);
    background: var(--surface-2);
    color: var(--text-2);
    font-weight: 700;
    text-align: start;
    vertical-align: bottom;
  }
  th.num {
    text-align: end;
  }
  th:first-child {
    border-start-start-radius: var(--r-sm);
    border-end-start-radius: var(--r-sm);
  }
  th:last-child {
    border-start-end-radius: var(--r-sm);
    border-end-end-radius: var(--r-sm);
  }
  /* 좁을 때: 머리 줄은 눈에서만 숨기고(보조 기술에는 남긴다), 줄이 카드로 쌓인다. 문턱은 칸 수에 따라 다르다(위). */
  @container table-few (max-width: 18rem) {
    thead {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip-path: inset(50%);
    }
    table,
    tbody {
      display: block;
    }
    table {
      width: auto;
      margin-inline: 0;
    }
  }
  @container table-many (max-width: 30rem) {
    thead {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip-path: inset(50%);
    }
    table,
    tbody {
      display: block;
    }
    table {
      width: auto;
      margin-inline: 0;
    }
  }
</style>
