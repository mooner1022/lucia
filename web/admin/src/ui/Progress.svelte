<script lang="ts">
  import type { Tone } from '../lib/types';

  // 쓴 양과 한도(사용량, 예산). 글로도 읽을 수 있게 label(막대 아래에 보이는 글)을 꼭 준다.
  // 보조 기술에 다른 이름을 줘야 하면 `name`(없으면 label이 이름이다).
  let {
    value,
    max,
    label,
    name,
    tone = 'info',
  }: { value: number; max: number; label: string; name?: string; tone?: Tone } = $props();
  const ratio = $derived(max > 0 ? Math.min(1, Math.max(0, value / max)) : 0);
</script>

<div class="wrap">
  <progress class={tone} value={ratio} max="1" aria-label={name ?? label}></progress>
  <span>{label}</span>
</div>

<style>
  .wrap {
    display: flex;
    flex-direction: column;
    gap: var(--s1);
    font-size: var(--fs-sm);
    color: var(--text-2);
  }
  progress {
    appearance: none;
    width: 100%;
    height: 0.5rem;
    border: 0;
    border-radius: var(--r-full);
    background: var(--surface-3);
    overflow: hidden;
  }
  progress::-webkit-progress-bar {
    background: var(--surface-3);
  }
  /* 채움은 쪽빛이다(띠의 색은 어두운 판의 홈과 대비가 모자란다). 결(ok·warn·bad)이 있으면 그 색이다. */
  progress::-webkit-progress-value {
    border-radius: var(--r-full);
    background: var(--accent);
  }
  progress::-moz-progress-bar {
    border-radius: var(--r-full);
    background: var(--accent);
  }
  .ok::-webkit-progress-value { background: var(--ok); }
  .ok::-moz-progress-bar { background: var(--ok); }
  .warn::-webkit-progress-value { background: var(--warn); }
  .warn::-moz-progress-bar { background: var(--warn); }
  .bad::-webkit-progress-value { background: var(--bad); }
  .bad::-moz-progress-bar { background: var(--bad); }
</style>
