<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { HTMLAnchorAttributes, HTMLButtonAttributes } from 'svelte/elements';
  import Icon from './Icon.svelte';

  // 단추. `href`를 주면 같은 모습의 링크가 된다. 폼 안에서 보내는 단추는 type="submit"을 준다.
  type Props = {
    variant?: 'default' | 'primary' | 'ghost' | 'danger' | 'ghost-danger';
    size?: 'md' | 'sm';
    icon?: string;
    block?: boolean;
    href?: string;
    children?: Snippet;
  } & Omit<HTMLButtonAttributes, 'children'> &
    Pick<HTMLAnchorAttributes, 'download' | 'target' | 'rel'>;

  let {
    variant = 'default',
    size = 'md',
    icon,
    block = false,
    href,
    type = 'button',
    children,
    download,
    target,
    rel,
    ...rest
  }: Props = $props();
</script>

{#if href}
  <a class="btn {variant} {size}" class:block {href} {download} {target} {rel}>
    {#if icon}<Icon name={icon} />{/if}
    {#if children}<span>{@render children()}</span>{/if}
  </a>
{:else}
  <button class="btn {variant} {size}" class:block {type} {...rest}>
    {#if icon}<Icon name={icon} />{/if}
    {#if children}<span>{@render children()}</span>{/if}
  </button>
{/if}

<style>
  .btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    max-width: 100%;
    /* 짧은 글("저장")의 알약이 동그라미가 되지 않게 너비의 바닥을 둔다. */
    min-width: 4.75rem;
    min-height: var(--control-h);
    padding: 0.4rem 1.1rem;
    border: 1px solid var(--border-strong);
    border-radius: var(--r-full);
    background: var(--surface);
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 600;
    line-height: 1.3;
    text-align: center;
    text-decoration: none;
    cursor: pointer;
    transition:
      background-color 0.15s,
      border-color 0.15s,
      box-shadow 0.15s;
  }
  .btn:hover {
    background: var(--surface-2);
  }
  .btn:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }
  .sm {
    min-height: var(--control-h-sm);
    min-width: 0;
    padding: 0.2rem 0.8rem;
  }
  .block {
    display: flex;
    width: 100%;
  }
  /* 주된 단추: 쪽빛 띠. 안쪽에 옅은 선을 한 줄 두른다(띠의 금선과 같은 꼴). */
  .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--on-accent);
    box-shadow:
      inset 0 0 0 2px var(--accent),
      inset 0 0 0 3px color-mix(in srgb, var(--on-accent) 38%, transparent),
      var(--shadow-band);
  }
  .primary:hover {
    background: var(--accent-hover);
    border-color: var(--accent-hover);
    box-shadow:
      inset 0 0 0 2px var(--accent-hover),
      inset 0 0 0 3px color-mix(in srgb, var(--on-accent) 38%, transparent),
      var(--shadow-band);
  }
  .ghost {
    border-color: transparent;
    background: transparent;
    color: var(--text-2);
  }
  .ghost:hover {
    background: var(--surface-3);
    color: var(--text);
  }
  .danger {
    background: var(--danger);
    border-color: var(--danger);
    color: var(--on-danger);
  }
  .danger:hover {
    background: var(--danger-hover);
    border-color: var(--danger-hover);
  }
  /* 지우기·끊기 같은 단추: 글만 붉게 하고 테두리는 여느 단추와 같게 둔다(흔한 단추 옆에서 더 무겁게 보이지 않게).
     실제로 보내기 전에 묻는 창이 한 번 더 뜬다. */
  .ghost-danger {
    color: var(--danger);
  }
  .ghost-danger:hover {
    background: var(--bad-soft);
  }
  @media (pointer: coarse) {
    .btn {
      min-height: 2.75rem;
    }
    .sm {
      min-height: 2.5rem;
    }
  }
  @media (forced-colors: active) {
    .btn {
      border-color: ButtonText;
    }
  }
  /* 인쇄에서는 배경이 찍히지 않는다. 색을 채운 단추의 글이 사라지지 않게 한다. */
  @media print {
    .primary,
    .danger {
      border-color: currentColor;
      background: none;
      color: inherit;
      box-shadow: none;
    }
  }
</style>
