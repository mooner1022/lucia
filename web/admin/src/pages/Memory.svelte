<script lang="ts">
  import type { PageData } from '../lib/types';
  import type { ExtraMemoryList } from '../lib/types.gen';
  import { ActionForm, Button, Columns, Empty, Item, Items, LastRun, Page, Section, Text, TextField, Time } from '../ui';

  // 기억: 찾기와 "기억해 둘 것 말하기"를 먼저, 그 아래에 갈래(나·주제·영역)별 항목 목록.
  let { data }: { data: PageData<ExtraMemoryList> } = $props();
  const s = $derived(data.snapshot);
  const query = $derived(data.extra.query);

  // 찾기: 서버는 거르지 않고 찾는 말(`query`)만 돌려준다. 제목·요약·세부 정보·이름에 들어 있으면 보인다(대소문자 무시).
  const needle = $derived(query.trim().toLowerCase());
  const shown = $derived(
    needle === ''
      ? s.memories
      : s.memories.filter((e) => [e.title, e.summary, e.body, e.slug].some((t) => t.toLowerCase().includes(needle))),
  );
</script>

<Page
  title="기억"
  desc="Lucia가 기억하는 항목입니다. 평소 대화에는 제목과 요약만 붙고, 세부 정보는 Lucia가 필요할 때 읽습니다."
>
  <Columns>
    <Section id="memory-search" title="찾기" icon="search" hint={`전체 ${s.memories.length}개`}>
      <ActionForm action="/memory" method="get" track={false} role="search">
        <TextField
          label="제목·요약·세부 정보에서 찾기"
          type="search"
          name="q"
          maxlength={100}
          placeholder="예: 강아지"
          value={query}
        >
          {#snippet action()}<Button type="submit" icon="search">찾기</Button>{/snippet}
        </TextField>
      </ActionForm>
      {#if needle !== ''}
        <Text size="sm" block>{shown.length}{'개 찾음 · '}<a href={data.path}>검색 지우기</a></Text>
      {/if}
    </Section>

    <Section id="memory-tell" title="기억해 둘 것 말하기" icon="chat">
      {#snippet lead()}Lucia가 알맞은 항목에 넣거나 고칩니다(모델 1회). 잊어 달라고 해도 됩니다.{/snippet}
      <ActionForm action="/memory/tell">
        <TextField
          label="Lucia에게 맡기기"
          name="text"
          required
          maxlength={1000}
          placeholder="예: 내 강아지 이름은 Beans야"
        >
          {#snippet action()}<Button type="submit" variant="primary">보내기</Button>{/snippet}
        </TextField>
      </ActionForm>
      <LastRun run={s.last_remember} />
    </Section>
  </Columns>

  {#if s.memories.length === 0}
    <Section id="memory-empty" title="기억 항목">
      <Empty>아직 기억한 것이 없습니다.</Empty>
    </Section>
  {:else if shown.length === 0}
    <Section id="memory-none" title="기억 항목">
      <Empty>찾는 말이 든 항목이 없습니다.</Empty>
    </Section>
  {/if}

  {#each data.consts.memory.sections as section (section.key)}
    {@const entries = shown.filter((e) => e.ui.section_key === section.key)}
    {#if entries.length > 0}
      <Section id={`memory-${section.key}`} title={section.label} hint={`${entries.length}개`}>
        <Items>
          {#each entries as e (e.slug)}
            <Item href={`/memory/${e.slug}`} title={e.title}>
              {e.summary}
              {#snippet end()}<Time at={e.updated_at} />{/snippet}
            </Item>
          {/each}
        </Items>
      </Section>
    {/if}
  {/each}
</Page>
