<script lang="ts">
  import type { HTMLTextareaAttributes } from 'svelte/elements';
  import './field.css';

  // 여러 줄 입력. `mono`는 고정폭(설정 글, 코드).
  type Props = {
    label: string;
    help?: string;
    mono?: boolean;
    hideLabel?: boolean;
  } & HTMLTextareaAttributes;

  let { label, help, mono = false, hideLabel = false, rows = 4, value = '', ...rest }: Props = $props();
  const id = $props.id();
</script>

<div class="field">
  <label for={id} class:sr-only={hideLabel}>{label}</label>
  <textarea {id} class="control" class:mono {rows} aria-describedby={help ? `${id}-help` : undefined} {...rest}>{value}</textarea>
  {#if help}<span class="help" id="{id}-help">{help}</span>{/if}
</div>
