<script lang="ts">
  // 긴 글·코드 상자(본문, 스크립트, 설정). 줄은 접혀서 가로로 넘치지 않고, 길면 상자 안에서 세로로 굴린다.
  // 길면 상자 안에서 굴려야 하므로 키보드로 닿을 수 있게 하고, 이름(`label`)을 붙인다.
  let { text, tall = false, label = '내용' }: { text: string; tall?: boolean; label?: string } = $props();
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<pre class:tall tabindex="0" role="region" aria-label={label}><code>{text}</code></pre>

<style>
  pre {
    max-height: 18rem;
    padding: var(--s3) var(--s4);
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    background: var(--surface-2);
    font-size: 0.8125rem;
    line-height: 1.6;
    overflow: auto;
  }
  .tall {
    max-height: 36rem;
  }
</style>
