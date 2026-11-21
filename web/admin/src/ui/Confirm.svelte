<script lang="ts">
  import type { Snippet } from 'svelte';
  import ActionForm from './ActionForm.svelte';
  import Button from './Button.svelte';

  // 되돌리기 어려운 동작: 첫 단추는 묻는 창을 열 뿐이고, 창 안의 단추가 실제로 보낸다.
  // 덧붙일 숨은 값은 `fields`에, 창 안에 더 보일 것(입력 등)은 children에 준다.
  let {
    action,
    label,
    question,
    confirmLabel = label,
    danger = false,
    size = 'sm',
    icon,
    fields = {},
    stay = false,
    children,
  }: {
    action: string;
    label: string;
    question: string;
    confirmLabel?: string;
    danger?: boolean;
    size?: 'md' | 'sm';
    icon?: string;
    fields?: Record<string, string>;
    /** 서버가 어디로 가라고 하든 지금 화면에 남는다(ActionForm의 stay). */
    stay?: boolean;
    children?: Snippet;
  } = $props();

  const uid = $props.id();
  let dialog: HTMLDialogElement | undefined = $state();

  function open() {
    if (!dialog) return;
    if (typeof dialog.showModal === 'function') dialog.showModal();
    else dialog.setAttribute('open', '');
  }

  function close() {
    if (!dialog) return;
    if (typeof dialog.close === 'function') dialog.close();
    else dialog.removeAttribute('open');
  }

  function backdrop(event: MouseEvent) {
    if (event.target === dialog) close();
  }
</script>

<Button variant={danger ? 'ghost-danger' : 'default'} {size} {icon} aria-haspopup="dialog" onclick={open}>{label}</Button>
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<dialog bind:this={dialog} aria-labelledby="{uid}-title" onclick={backdrop}>
  <div class="box">
    <h2 id="{uid}-title">{label}</h2>
    <p>{question}</p>
    <ActionForm {action} track={false} {stay} onsend={close}>
      {#each Object.entries(fields) as [name, value] (name)}
        <input type="hidden" {name} {value} />
      {/each}
      {@render children?.()}
      <div class="actions">
        <Button variant="ghost" onclick={close}>닫기</Button>
        <Button type="submit" variant={danger ? 'danger' : 'primary'}>{confirmLabel}</Button>
      </div>
    </ActionForm>
  </div>
</dialog>

<style>
  /* 둥근 창(--r-lg, --shadow-2). 좁은 화면(360px)에서도 양옆에 1rem씩 남는다. */
  dialog {
    width: min(28rem, calc(100vw - 2rem));
    max-height: calc(100dvh - 2rem);
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-2);
    overflow: auto;
  }
  dialog::backdrop {
    background: var(--scrim);
  }
  .box {
    display: flex;
    flex-direction: column;
    gap: var(--s3);
    padding: var(--s5);
  }
  h2 {
    font-size: var(--fs-lg);
    overflow-wrap: anywhere;
  }
  p {
    color: var(--text-2);
    overflow-wrap: anywhere;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--s2);
  }
</style>
