<script lang="ts">
  // 그림(이모지·스티커, 만든 그림). 정해진 크기 안에 비율을 지켜 들어간다. alt는 꼭 준다(장식이면 빈 글).
  let { src, alt, size = 'md' }: { src: string; alt: string; size?: 'sm' | 'md' | 'lg' } = $props();
</script>

<img class={size} {src} {alt} loading="lazy" decoding="async" />

<style>
  img {
    display: block;
    flex: none;
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    background: var(--surface-2);
    object-fit: contain;
  }
  .sm { width: 2.5rem; height: 2.5rem; }
  .md { width: 4.5rem; height: 4.5rem; }
  .lg { width: 9rem; height: 9rem; }
</style>
