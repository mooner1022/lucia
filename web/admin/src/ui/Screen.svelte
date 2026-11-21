<script lang="ts">
  import { untrack } from 'svelte';
  import { submit } from '../lib/app.svelte';

  // 원격 화면(브라우저 화면 넘겨받기). 그림을 누르면 그 자리의 좌표를 서버로 보낸다.
  // 그림은 자리에 맞춰 줄어들고, 누른 자리는 **원래 그림의 좌표**로 바꿔 보낸다(휴대폰에서도 옆으로 밀지 않고 누를 수 있다).
  // 폼은 예전과 같은 꼴이다: `<input type="image" name="at">`이 `at.x`, `at.y`를 보낸다. 숨은 값은 `fields`에 준다.
  let {
    action,
    src,
    alt,
    fields = {},
  }: { action: string; src: string; alt: string; fields?: Record<string, string> } = $props();

  let busy = $state(false);

  /** 그림의 원래 크기. 화면에 놓인 `<input type="image">`에서는 읽을 수 없어 같은 주소를 따로 읽어 잰다. */
  function measure(address: string): Promise<{ w: number; h: number } | null> {
    return new Promise((resolve) => {
      const probe = new Image();
      const done = () => resolve(probe.naturalWidth > 0 ? { w: probe.naturalWidth, h: probe.naturalHeight } : null);
      probe.onload = done;
      probe.onerror = () => resolve(null);
      probe.src = address;
      if (probe.naturalWidth > 0) done();
      // 끝내 못 재면 보이는 크기 그대로 보낸다(누름을 붙잡아 두지 않는다).
      setTimeout(() => resolve(null), 3000);
    });
  }

  // 처음 그릴 때 한 번 재 둔다(원격 화면의 크기는 넘겨받는 동안 바뀌지 않는다). 누를 때 아직 못 쟀으면 그때 잰다.
  let natural: Promise<{ w: number; h: number } | null> | null = null;
  $effect(() => {
    natural = measure(untrack(() => src));
  });

  async function press(event: MouseEvent) {
    event.preventDefault();
    // 키보드로 누른 것(Enter·Space)에는 자리가 없다. 화면 왼쪽 위 (0,0)을 누르게 두지 않는다(키는 아래 단추로 보낸다).
    if (event.detail === 0) return;
    const image = event.currentTarget as HTMLInputElement;
    const form = image.form;
    if (!form || busy) return;
    const box = image.getBoundingClientRect();
    const point = { x: event.clientX - box.left, y: event.clientY - box.top };
    busy = true;
    try {
      const size = natural ? await natural : await measure(image.src);
      const scaleX = size && box.width > 0 ? size.w / box.width : 1;
      const scaleY = size && box.height > 0 ? size.h / box.height : 1;
      const x = Math.max(0, Math.round(point.x * scaleX));
      const y = Math.max(0, Math.round(point.y * scaleY));
      await submit(form, null, { 'at.x': String(x), 'at.y': String(y) });
    } finally {
      busy = false;
    }
  }
</script>

<form method="post" {action} aria-busy={busy} onsubmit={(event) => event.preventDefault()}>
  {#each Object.entries(fields) as [name, value] (name)}
    <input type="hidden" {name} {value} />
  {/each}
  <input type="image" name="at" {src} {alt} onclick={press} />
</form>

<style>
  form {
    margin: 0;
    border: 1px solid var(--border-strong);
    border-radius: var(--r-md);
    background: var(--surface-2);
    box-shadow: var(--shadow-1);
    overflow: hidden;
    line-height: 0;
  }
  input[type='image'] {
    display: block;
    width: 100%;
    height: auto;
    cursor: crosshair;
  }
  form[aria-busy='true'] {
    opacity: 0.7;
  }
</style>
