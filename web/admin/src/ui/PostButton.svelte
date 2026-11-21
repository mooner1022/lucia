<script lang="ts">
  import type { Snippet } from 'svelte';
  import ActionForm from './ActionForm.svelte';
  import Button from './Button.svelte';

  // 단추 하나짜리 폼(지금 실행, 켜기, 멈추기 …). 덧붙일 숨은 값은 `fields`에 준다.
  let {
    action,
    variant = 'default',
    size = 'sm',
    icon,
    fields = {},
    stay = false,
    children,
  }: {
    action: string;
    variant?: 'default' | 'primary' | 'ghost' | 'danger' | 'ghost-danger';
    size?: 'md' | 'sm';
    icon?: string;
    fields?: Record<string, string>;
    /** 서버가 어디로 가라고 하든 지금 화면에 남는다(ActionForm의 stay). */
    stay?: boolean;
    children?: Snippet;
  } = $props();
</script>

<ActionForm {action} inline track={false} {stay}>
  {#each Object.entries(fields) as [name, value] (name)}
    <input type="hidden" {name} {value} />
  {/each}
  <Button type="submit" {variant} {size} {icon}>{@render children?.()}</Button>
</ActionForm>
