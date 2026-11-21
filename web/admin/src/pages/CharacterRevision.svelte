<script lang="ts">
  import type { PageData } from '../lib/types';
  import type { ExtraCharacterRevision, ExtraCharacterRevisionMissing } from '../lib/types.gen';
  import { Badge, CodeBlock, Confirm, Empty, Page, Section, Time } from '../ui';

  // 캐릭터 파일의 예전 개정 하나: 내용과 되돌리기. 지금 내용과 같은 개정이면 되돌리기 대신 표지를 보인다.
  let { data }: { data: PageData<ExtraCharacterRevision | ExtraCharacterRevisionMissing> } = $props();
  const found = $derived.by(() => {
    const extra = data.extra;
    return 'revision' in extra ? extra : null;
  });
  const back = $derived({ href: data.path, label: '캐릭터' });
</script>

{#if found}
  {@const r = found.revision}
  <Page title={`개정 ${r.id}`} {back}>
    {#snippet lead()}<Time at={r.at} />{' · '}{r.note}{/snippet}
    {#snippet actions()}
      {#if found.is_current}
        <Badge tone="ok">지금 내용</Badge>
      {:else}
        <Confirm
          action={`/character/rev/${r.id}/restore`}
          label="이 내용으로 되돌리기"
          question="지금 캐릭터 파일을 이 개정의 내용으로 바꿉니다. 지금 내용도 기록에 남습니다. 상주 대화에는 다음 세션 정리나 새 세션부터 적용됩니다."
          confirmLabel="되돌리기"
        />
      {/if}
    {/snippet}

    <Section id="revision" title="내용" icon="chat">
      <CodeBlock text={r.content} tall />
    </Section>
  </Page>
{:else}
  <Page title="캐릭터 개정" {back}>
    <Section id="revision-missing" title="찾을 수 없음">
      <Empty>없는 개정입니다.</Empty>
    </Section>
  </Page>
{/if}
