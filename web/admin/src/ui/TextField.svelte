<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { HTMLInputAttributes } from 'svelte/elements';
  import './field.css';

  // 한 줄 입력(type="file"·"password"·"number"도 이것으로). name·type·min·max·maxlength·required·inputmode·
  // autocomplete·accept 같은 속성은 그대로 input에 간다. `action`에 단추를 주면 입력 옆에 놓인다(좁으면 아래로).
  type Props = {
    label: string;
    help?: string;
    width?: 'xs' | 'sm' | 'md' | 'full';
    grow?: boolean;
    hideLabel?: boolean;
    action?: Snippet;
  } & HTMLInputAttributes;

  let { label, help, width = 'full', grow = false, hideLabel = false, action, type = 'text', ...rest }: Props = $props();
  const id = $props.id();
</script>

<div class="field w-{width}" class:grow>
  <label for={id} class:sr-only={hideLabel}>{label}</label>
  <div class="row">
    <input {id} class="control" {type} aria-describedby={help ? `${id}-help` : undefined} {...rest} />
    {#if action}{@render action()}{/if}
  </div>
  {#if help}<span class="help" id="{id}-help">{help}</span>{/if}
</div>
