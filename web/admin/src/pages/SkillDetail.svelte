<script lang="ts">
  import { exact } from '../lib/format';
  import type { PageData } from '../lib/types';
  import type { ExtraSkillsDetail } from '../lib/types.gen';
  import { CodeBlock, Disclosure, KV, KVRow, Page, PostButton, Section, Stack, Text } from '../ui';

  // 스킬 하나: 정보(이름, 상태, 언제, 쓰는 것, 쓴 횟수, 출처)와 개정별 본문·스크립트.
  let { data }: { data: PageData<ExtraSkillsDetail> } = $props();
  const sk = $derived(data.extra.skill);
  const tz = $derived(data.snapshot.tz);

  // 비어 올 수 있는 칸(Option)은 생성된 타입에 없어 여기서 좁힌다.
  const scriptTools = $derived<string[] | null>(sk.script_tools);
  const taskId = $derived(typeof sk.task_id === 'string' ? sk.task_id : null);
</script>

<Page title={sk.title} desc={`[[${sk.slug}]]`} back={{ href: data.path, label: '스킬 목록' }}>
  {#snippet actions()}
    {#if sk.ui.can_approve}<PostButton action={`/skills/${sk.slug}/approve`} variant="primary">승인</PostButton>{/if}
    {#if sk.pinned}
      <PostButton action={`/skills/${sk.slug}/unpin`}>고정 풀기</PostButton>
    {:else}
      <PostButton action={`/skills/${sk.slug}/pin`}>고정</PostButton>
    {/if}
    {#if sk.ui.can_restore}
      <PostButton action={`/skills/${sk.slug}/restore`}>되살리기</PostButton>
    {:else}
      <PostButton action={`/skills/${sk.slug}/archive`}>끄기</PostButton>
    {/if}
  {/snippet}

  <Section id="skill-info" title="정보" icon="spark">
    <KV>
      <KVRow label="이름">[[{sk.slug}]]</KVRow>
      <KVRow label="상태">{sk.ui.status_label}{#if sk.pinned}{' · 고정'}{/if}</KVRow>
      <KVRow label="언제">{sk.description}</KVRow>
      <KVRow label="쓰는 것">{sk.ui.footprint}</KVRow>
      <KVRow label="쓴 횟수">{sk.uses}{' (확인된 성공 '}{sk.successes}{')'}</KVRow>
      {#if scriptTools !== null}
        <KVRow label="단축키 스크립트">있음 · 도구: {scriptTools.length === 0 ? '없음' : scriptTools.join(', ')}</KVRow>
      {/if}
      <KVRow label="출처">{sk.ui.source_label}{#if taskId !== null}{' · '}{taskId}{/if}</KVRow>
    </KV>
  </Section>

  <Section id="skill-revisions" title="개정" icon="clock">
    <!-- 지금 개정만 펼쳐 둔다. 시각은 분까지 적는다(데몬의 시간대). -->
    {#each data.extra.revisions as r (r.revision)}
      <Disclosure
        summary={`개정 ${r.revision} · ${exact(r.at, tz).slice(0, 16)} · ${r.ui.author_label} · ${r.reason}`}
        open={r.revision === sk.revision}
      >
        <CodeBlock text={r.body} />
        {#if r.script !== null}
          <Stack gap={2}>
            <Text size="sm" tone="muted" block>스크립트(run.py)</Text>
            <CodeBlock text={r.script} />
          </Stack>
        {/if}
      </Disclosure>
    {/each}
  </Section>
</Page>
