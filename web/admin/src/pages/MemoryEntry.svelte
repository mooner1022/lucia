<script lang="ts">
  import type { PageData } from '../lib/types';
  import type { ExtraMemoryEntry, ExtraMemoryMissing } from '../lib/types.gen';
  import { Confirm, Empty, Item, Items, KV, KVRow, Page, Section, Time } from '../ui';
  import { parseMemoryText, type Inline } from './MemoryEntryText';

  // 기억 항목 하나: 내용(요약·세부 정보), 변경 기록(예전 개정으로 되돌리기), 지우기. 없는 항목이면 그 안내.
  let { data }: { data: PageData<ExtraMemoryEntry | ExtraMemoryMissing> } = $props();
  const found = $derived.by(() => {
    const extra = data.extra;
    return 'entry' in extra ? extra : null;
  });
  const body = $derived(found ? parseMemoryText(found.entry.body) : []);
  const back = $derived({ href: data.path, label: '기억' });
</script>

{#snippet inline(parts: Inline[])}
  {#each parts as part}{#if part.link}<a href={`/memory/${part.text}`}>{part.text}</a>{:else}{part.text}{/if}{/each}
{/snippet}

{#if found}
  {@const entry = found.entry}
  <Page title={entry.title} desc={`${entry.ui.section_label} · [[${entry.slug}]]`} {back}>
    {#snippet actions()}
      <Confirm
        action={`/memory/${entry.slug}/delete`}
        label="지우기"
        question="이 항목을 지웁니다. 지운 내용은 변경 기록에 남지만, Lucia는 더 이상 이 항목을 보지 못합니다."
        confirmLabel="지우기"
        danger
      />
    {/snippet}

    <Section id="entry" title="내용" icon="memory">
      <KV>
        <KVRow label="마지막 업데이트"><Time at={entry.updated_at} /></KVRow>
        <KVRow label="요약">{entry.summary}</KVRow>
      </KV>
      <h3>세부 정보</h3>
      {#if entry.body.trim() === ''}
        <Empty>세부 정보가 없습니다.</Empty>
      {:else}
        {#each body as block}
          {#if block.kind === 'list'}
            <ul>
              {#each block.items as item}<li>{@render inline(item)}</li>{/each}
            </ul>
          {:else if block.kind === 'heading'}
            <h4>{@render inline(block.content)}</h4>
          {:else}
            <p>{@render inline(block.content)}</p>
          {/if}
        {/each}
      {/if}
    </Section>

    <Section id="history" title="변경 기록" icon="clock">
      {#snippet lead()}Lucia가 바꾼 것이 틀렸으면 대화에서 바로 고쳐 달라고 하거나, 여기서 예전 내용으로 되돌립니다.{/snippet}
      <Items>
        {#each found.history as r, i (r.id)}
          {#snippet restore()}
            <!-- 맨 위 개정이 지금 내용이다. 그보다 예전이면서 내용이 있는(지움이 아닌) 개정만 되돌릴 수 있다. -->
            {#if i > 0 && !r.ui.deleted}
              <Confirm
                action={`/memory/${entry.slug}/rev/${r.id}/restore`}
                label="이 내용으로 되돌리기"
                question="이 개정의 제목·요약·세부 정보로 항목을 되돌립니다. 지금 내용은 변경 기록에 남습니다."
                confirmLabel="되돌리기"
              />
            {/if}
          {/snippet}
          <Item
            title={`${r.ui.author_label} · ${r.reason}`}
            icon={r.ui.deleted ? 'x-circle' : 'clock'}
            tone={r.ui.deleted ? 'bad' : 'muted'}
            end={restore}
          >
            <p>
              {r.ui.origin}{' · '}<Time at={r.at} />{' · 개정 '}{r.id}{#if r.ui.deleted}{' · 지움'}{/if}
            </p>
            {#if !r.ui.deleted}<p>요약: {r.summary}</p>{/if}
          </Item>
        {/each}
      </Items>
    </Section>
  </Page>
{:else}
  <Page title="기억 항목" {back}>
    <Section id="memory-missing" title="찾을 수 없음">
      <Empty>없는 항목입니다. 지워졌거나 이름이 틀렸습니다.</Empty>
    </Section>
  </Page>
{/if}
