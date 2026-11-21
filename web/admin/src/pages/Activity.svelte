<script lang="ts">
  import type { PageData } from '../lib/types';
  import type { ExtraActivity } from '../lib/types.gen';
  import { Badge, Callout, Columns, Empty, Item, Items, Page, Section, Tabs, Time } from '../ui';

  // 활동: 먼저 내가 눌러야 할 승인 대기, 그다음 에이전트가 한 일의 기록(종류 거르개)이 주인공이고,
  // 메일 발송 기록·알림·최근 신호는 옆에 두고 훑는 보조 기록이다.
  let { data }: { data: PageData<ExtraActivity> } = $props();
  const s = $derived(data.snapshot);

  // 화면 주소는 서버가 정한다(consts.pages). 모르는 키면 홈으로 둔다.
  const pathOf = (key: string) => data.consts.pages.find((p) => p.key === key)?.path ?? '/';

  // 승인 대기는 열거형 원본이 아니라 서버가 준 불리언으로 가른다.
  const pending = $derived(s.approvals.filter((a) => a.ui.pending));
  // 고른 종류에 속한 활동만(속한 거르개는 서버가 ui.groups로 준다).
  const shown = $derived(s.activity.filter((a) => a.ui.groups.includes(data.extra.filter)));
  const filterTabs = $derived(
    data.consts.activity_filters.map((f) => ({
      href: `/activity?kind=${f.key}`,
      label: f.label,
      current: f.key === data.extra.filter,
    })),
  );

  // 신호 요약은 80글자(유니코드 글자 단위)까지만 보인다.
  const clip = (text: string, n: number) => [...text].slice(0, n).join('');
</script>

<Page title="활동" desc="에이전트가 한 일과 받은 허락, 보내지 못한 알림을 봅니다.">
  {#if pending.length > 0}
    <Section id="pending" title="승인 대기" icon="send" tone="warn">
      {#snippet end()}<Badge tone="info">{pending.length}건</Badge>{/snippet}
      <Callout tone="info">
        Discord DM의 카드에서 <strong>[보내기]</strong> 또는 <strong>[거절]</strong>을 누르세요. 답이 없으면 {s.approval_ttl_hours}시간 뒤
        만료됩니다.
      </Callout>
      <Items>
        {#each pending as a (a.id)}
          <Item title={a.summary} icon="mail" tone="info">
            요청 <Time at={a.requested_at} /> · 만료 <Time at={a.expires_at} />
          </Item>
        {/each}
      </Items>
    </Section>
  {/if}

  <Section id="log" title="활동 기록" icon="activity" hint={`최근 ${s.activity.length}건 중 ${shown.length}건`}>
    <Tabs label="종류 거르기" items={filterTabs} />
    {#if shown.length === 0}
      <Empty>이 종류의 활동이 없습니다.</Empty>
    {:else}
      <Items>
        {#each shown as a (a.id)}
          <Item title={a.ui.label} icon={a.ui.icon} tone={a.ui.tone}>
            {a.ui.detail}
            {#snippet end()}<Time at={a.at} />{/snippet}
          </Item>
        {/each}
      </Items>
    {/if}
  </Section>

  <Columns>
    <Section id="sends" title="메일 발송 기록" icon="send">
      {#if s.approvals.length === 0}
        <Empty>아직 발송 요청이 없습니다.</Empty>
      {:else}
        <Items>
          {#each s.approvals as a (a.id)}
            {#snippet sendHeading()}<Badge tone={a.ui.tone}>{a.ui.status}</Badge> {a.summary}{/snippet}
            <Item heading={sendHeading}>
              {#if a.result !== null}{a.result}{/if}
              {#snippet end()}<Time at={a.requested_at} />{/snippet}
            </Item>
          {/each}
        </Items>
      {/if}
    </Section>

    <Section id="alerts" title="알림" icon="bell">
      {#if s.alerts.length === 0}
        <Empty>최근 알림이 없습니다.</Empty>
      {:else}
        <Items>
          {#each s.alerts as a (a.id)}
            <Item title={a.text} icon="bell" tone={a.delivered ? 'info' : 'warn'}>
              <Time at={a.at} />{#if !a.delivered}{' · Discord로 보내지 못함'}{/if}
            </Item>
          {/each}
        </Items>
      {/if}
    </Section>

    <!-- 새 메일·일정 신호의 최근 기록(결정 0031). 설정은 알림 설정에서 한다. -->
    <Section
      id="signals"
      title="최근 신호"
      icon="mail"
      hint={`오늘 분류 ${s.signals.classify_used}/${s.signals.classify_daily} · 깨우기 ${s.signals.wake_used}/${s.signals.wake_daily}`}
    >
      {#snippet end()}<a href={pathOf('notify')}>알림 설정</a>{/snippet}
      {#if s.signals.recent.length === 0}
        <Empty>아직 분류한 신호가 없습니다. 새 메일·일정이 확인되면 여기에 "지금 알림 / 소식 / 버림"으로 나뉘어 남습니다.</Empty>
      {:else}
        <Items>
          {#each s.signals.recent as x (x.id)}
            <Item title={clip(x.summary, 80)}>
              {x.ui.verdict_label}
              {#if x.reason !== null}{' · '}{x.reason}{/if}
              {#if x.interest !== null}{' · 관심: '}{x.interest}{/if}
              {' · '}<Time at={x.received_at} />
            </Item>
          {/each}
        </Items>
      {/if}
    </Section>
  </Columns>
</Page>
