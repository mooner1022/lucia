<script lang="ts">
  import Tabs from './Tabs.svelte';

  // 한 화면 안의 구역으로 건너뛰는 링크(구역 Section의 id와 그 안의 건수).
  let { items }: { items: { id: string; label: string; count?: number }[] } = $props();
</script>

<Tabs label="이 화면의 구역" items={items.map((i) => ({ href: `#${i.id}`, label: i.label, count: i.count }))} />
