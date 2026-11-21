<script lang="ts">
  import { app } from '../lib/app.svelte';
  import { exact, when } from '../lib/format';

  // 시각: 보이는 글은 상대 표기("3분 전", "오늘 14:00"), 정확한 시각은 title. 데몬의 시간대로 적는다.
  let { at }: { at: string } = $props();
  const page = $derived(app.data?.kind === 'page' ? app.data : null);
  const tz = $derived((page?.snapshot.tz as string | undefined) ?? 'Asia/Seoul');
  const now = $derived((page?.snapshot.now as string | undefined) ?? new Date().toISOString());
</script>

<time datetime={at} title={exact(at, tz)}>{when(at, now, tz)}</time>
