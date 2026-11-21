<script lang="ts">
  import type { PageData } from '../lib/types';
  import type { ExtraSkillsList } from '../lib/types.gen';
  import { Badge, Disclosure, Empty, Item, Items, Page, PostButton, Section, Text } from '../ui';

  // 스킬 목록: 승인해야 하는 것을 위로, 각 줄에서 승인·고정·끄기(되살리기)를 바로 한다.
  let { data }: { data: PageData<ExtraSkillsList> } = $props();
  type Skill = ExtraSkillsList['skills'][number];

  // 승인 대기를 먼저 둔다(정렬은 안정적이라 나머지는 서버가 준 순서 그대로다).
  const skills = $derived([...data.extra.skills].sort((a, b) => Number(b.ui.can_approve) - Number(a.ui.can_approve)));
  const usable = $derived(data.extra.skills.filter((sk) => sk.ui.usable).length);
</script>

{#snippet actions(sk: Skill)}
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

<Page title="스킬" desc="Lucia가 성공한 작업에서 정리한 절차입니다. 비슷한 일을 할 때 읽고 따라 더 적은 단계로 끝냅니다.">
  <Section id="skills" title="스킬 목록" icon="spark" hint={`${usable}개 사용 중`}>
    {#if skills.length === 0}
      <Empty>아직 스킬이 없습니다. Lucia에게 "이거 스킬로 만들어"라고 하거나, 맡긴 작업을 [완료]로 확인하면 회고가 만듭니다.</Empty>
    {:else}
      <Items>
        {#each skills as sk (sk.slug)}
          {#snippet heading()}
            <a href={`/skills/${sk.slug}`}>{sk.title}</a>
            <Text size="sm" tone="muted">[[{sk.slug}]]</Text>
          {/snippet}
          {#snippet end()}{@render actions(sk)}{/snippet}
          <Item {heading} icon="spark" tone={sk.ui.tone} {end}>
            <p>
              <Badge tone={sk.ui.tone}>{sk.ui.status_label}</Badge>
              {#if sk.pinned}{' '}<Badge tone="ok">고정</Badge>{/if}
              {' 쓴 횟수 '}{sk.uses}{' · 확인된 성공 '}{sk.successes}{' · '}{sk.ui.footprint}
            </p>
            <p>{sk.description}</p>
          </Item>
        {/each}
      </Items>
    {/if}
    <Disclosure summary="스킬은 어떻게 쓰이나">
      <p>
        읽기만 하는 절차는 시험 중으로 시작해, 그것을 쓴 작업을 [완료]로 두 번 확인하면 활성이 됩니다. 사이트를 열거나 외부에 쓰는 절차는
        승인해야 씁니다.
      </p>
      <p>60일 안 쓰면 오래됨, 그 뒤 60일이면 보관합니다(고정한 것은 빼고). 스킬은 권한을 넓히지 않습니다.</p>
    </Disclosure>
  </Section>
</Page>
