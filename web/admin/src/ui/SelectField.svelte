<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { HTMLSelectAttributes } from 'svelte/elements';
  import './field.css';

  // 고르기. options의 value가 서버로 가는 값이다. `action`에 단추를 주면 옆에 놓인다(좁으면 아래로).
  type Props = {
    label: string;
    help?: string;
    options: { value: string; label: string; disabled?: boolean }[];
    value?: string;
    width?: 'xs' | 'sm' | 'md' | 'full';
    grow?: boolean;
    hideLabel?: boolean;
    action?: Snippet;
  } & Omit<HTMLSelectAttributes, 'value'>;

  let {
    label,
    help,
    options,
    value = '',
    width = 'full',
    grow = false,
    hideLabel = false,
    action,
    ...rest
  }: Props = $props();
  const id = $props.id();
</script>

<div class="field w-{width}" class:grow>
  <label for={id} class:sr-only={hideLabel}>{label}</label>
  <div class="row">
    <select {id} class="control" aria-describedby={help ? `${id}-help` : undefined} {...rest}>
      {#each options as option (option.value)}
        <option value={option.value} selected={option.value === value} disabled={option.disabled}>{option.label}</option>
      {/each}
    </select>
    {#if action}{@render action()}{/if}
  </div>
  {#if help}<span class="help" id="{id}-help">{help}</span>{/if}
</div>
