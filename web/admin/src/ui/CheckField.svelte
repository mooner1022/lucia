<script lang="ts">
  import type { HTMLInputAttributes } from 'svelte/elements';

  // 체크 상자(또는 type="radio") 한 줄. 라벨 전체를 누를 수 있다.
  type Props = {
    label: string;
    help?: string;
    type?: 'checkbox' | 'radio';
  } & Omit<HTMLInputAttributes, 'type'>;

  let { label, help, type = 'checkbox', value = '1', ...rest }: Props = $props();
  const id = $props.id();
</script>

<div class="check">
  <label for={id}>
    <input {id} {type} {value} aria-describedby={help ? `${id}-help` : undefined} {...rest} />
    <span>{label}</span>
  </label>
  {#if help}<span class="help" id="{id}-help">{help}</span>{/if}
</div>

<style>
  .check {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
  }
  /* 가리키면 줄 전체에 옅은 쪽빛 번짐. 번짐이 상자 밖으로 번지도록 안쪽 여백을 주고, 같은 만큼 바깥 여백을 빼서
     체크 상자와 글의 자리는 다른 입력과 같게 둔다. */
  label {
    display: flex;
    align-items: flex-start;
    gap: 0.6rem;
    min-height: 1.75rem;
    margin-inline: -0.5rem;
    padding: 0.1rem 0.5rem;
    border-radius: var(--r-sm);
    cursor: pointer;
    transition: background-color 0.15s;
  }
  label:hover {
    background: color-mix(in srgb, var(--accent) 6%, transparent);
  }
  input {
    flex: none;
    width: 1.15rem;
    height: 1.15rem;
    margin: 0.22rem 0 0;
    accent-color: var(--accent);
  }
  .help {
    padding-inline-start: 1.75rem;
    font-size: var(--fs-sm);
    color: var(--text-3);
  }
  @media (pointer: coarse) {
    label {
      min-height: 2.75rem;
      align-items: center;
    }
    input {
      margin-top: 0;
    }
  }
</style>
