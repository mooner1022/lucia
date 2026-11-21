<script lang="ts">
  import { usd } from '../lib/format';
  import type { PageData, Tone } from '../lib/types';
  import type { Consts } from '../lib/types.gen';
  import {
    Badge,
    Callout,
    Columns,
    Confirm,
    Empty,
    Grid,
    Item,
    Items,
    LastRun,
    Page,
    PostButton,
    Section,
    Stat,
    Text,
    Time,
  } from '../ui';

  // 홈: 먼저 "다 괜찮은가, 내가 할 것이 있나"(확인할 것, 승인 대기), 그다음 상태 타일, 곧 올 일·최근 활동,
  // 손으로 쓰는 것(권한 모드, 지금 실행), 사용량 순서다. 오래 걸리는 관리는 각 화면에서 하고 여기는 요약과 길 안내다.
  let { data }: { data: PageData } = $props();
  const s = $derived(data.snapshot);
  // PageData 형에 consts가 아직 없어 여기서 좁힌다.
  const consts = $derived((data as PageData & { consts: Consts }).consts);

  const toneIcon: Record<Tone, string> = { ok: 'check-circle', bad: 'x-circle', warn: 'alert', info: 'info', muted: 'info' };
  // 화면 주소는 서버가 정한다(consts.pages). 모르는 키면 홈으로 둔다.
  const pathOf = (key: string) => consts.pages.find((p) => p.key === key)?.path ?? '/';

  // 승인 대기는 열거형 원본이 아니라 서버가 준 불리언으로 가른다.
  const pending = $derived(s.approvals.filter((a) => a.ui.pending));

  // 상태 타일
  const failed = $derived(s.checks.filter((c) => !c.ok).length);
  // 다음 루틴: 켜짐 여부는 보지 않고 다음 실행 시각이 가장 이른 것(같으면 먼저 나온 것). 시각은 Rust에서 Option이다.
  const next = $derived.by(() => {
    let best: { at: string; label: string } | null = null;
    for (const r of s.schedules) {
      const at: string | null = r.next_due_at;
      if (at && (best === null || Date.parse(at) < Date.parse(best.at))) best = { at, label: r.ui.label };
    }
    return best;
  });

  // 곧 올 일: 후속 조치, 챙기는 마감, 진행 중인 목표의 다음 점검을 시각순으로 섞는다.
  type Upcoming = { key: string; at: string; kind: string; title: string; icon: string };
  const upcoming = $derived.by(() => {
    const rows: Upcoming[] = [];
    for (const f of s.followups_open) rows.push({ key: `f-${f.id}`, at: f.due_at, kind: '후속 조치', title: f.task, icon: 'clock' });
    for (const n of s.nudges) rows.push({ key: `n-${n.id}`, at: n.due_at, kind: '챙기는 마감', title: n.title, icon: 'flag' });
    for (const v of s.goals) {
      if (v.ui.open && v.goal.next_review_at) {
        rows.push({ key: `g-${v.goal.id}`, at: v.goal.next_review_at, kind: '목표 점검', title: v.goal.title, icon: 'tasks' });
      }
    }
    return rows.sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
  });
  const openGoals = $derived(s.goals.filter((g) => g.ui.open).length);

  // 권한 모드: 지금 모드는 consts.security.modes의 key와 맞춘다.
  const mode = $derived(s.ui.security_mode);
  const modeNow = $derived(consts.security.modes.find((m) => m.key === mode));
</script>

<Page title="홈" desc="지금 상태와 손볼 것을 한눈에 봅니다.">
  <!-- 대화할 곳이 아직 없고 안내를 끄지 않았을 때만(이미 설정한 설치에는 보이지 않는다, 결정 0064). -->
  {#if !s.setup.complete && !s.setup.dismissed}
    <Callout tone="info" title="초기 설정이 남아 있습니다.">
      대화할 곳(Discord나 앱)이 아직 없습니다. <a href={pathOf('start')}>초기 설정</a>에서 이름과 연결을 정할 수 있습니다.
    </Callout>
  {/if}
  <Section
    id="attention"
    title="확인할 것"
    icon="bell"
    hint={data.notices.length > 0 ? `${data.notices.length}건` : undefined}
    tone={data.overall.tone}
  >
    {#snippet end()}<Badge tone={data.overall.tone}>{data.overall.label}</Badge>{/snippet}
    {#if data.notices.length === 0}
      <Callout tone="ok">손볼 것이 없습니다. 모두 정상입니다.</Callout>
    {:else}
      <Items>
        {#each data.notices as n}
          <Item icon={toneIcon[n.tone]} tone={n.tone} href={n.page} title={n.text} />
        {/each}
      </Items>
    {/if}
  </Section>

  {#if pending.length > 0}
    <Section id="pending" title="승인 대기" icon="send">
      {#snippet end()}
        <Badge tone="info">{pending.length}건</Badge>
        <a href={pathOf('activity')}>활동에서 보기</a>
      {/snippet}
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

  <Grid min="xs" gap={3} stretch>
    <Stat label="Discord" value={s.discord.ui.status} tone={s.discord.ui.tone} href={pathOf('connections')}>
      {#if s.discord.ui.bot}{s.discord.ui.bot} · {/if}{s.discord.owner !== null ? '소유자 설정됨' : '소유자 없음'}
    </Stat>
    <Stat label="Google" value={s.google.ui.status} tone={s.google.ui.tone} href={pathOf('connections')}>
      {#if s.google.connected}
        {s.google.account ?? '계정 확인 안 됨'}
      {:else if s.google.has_client}
        계정 동의가 필요합니다
      {:else}
        메일·일정을 보려면 연결하세요
      {/if}
    </Stat>
    {#if s.checks.length === 0}
      <Stat label="실행 환경" value="점검 전" tone="muted" href={pathOf('system')}>시스템 화면에서 점검합니다</Stat>
    {:else if failed === 0}
      <Stat label="실행 환경" value="모두 정상" tone="ok" href={pathOf('system')}>점검 {s.checks.length}개 통과</Stat>
    {:else}
      <Stat label="실행 환경" value="{failed}개 실패" tone="bad" href={pathOf('system')}>점검 {s.checks.length}개 중</Stat>
    {/if}
    {#if next}
      <Stat label="다음 루틴" value={next.label} href={pathOf('system')}><Time at={next.at} /></Stat>
    {:else}
      <Stat label="다음 루틴" value="없음" href={pathOf('system')}>켜진 루틴이 없습니다</Stat>
    {/if}
  </Grid>

  <Columns>
      <Section id="upcoming" title="곧 올 일" icon="clock">
        {#snippet end()}<a href={pathOf('work')}>할 일에서 관리</a>{/snippet}
        <Text tone="muted" size="sm" block>
          목표 {openGoals}개 · 작업 {s.tasks_open.length}건 · 후속 조치 {s.followups_open.length}건 · 챙기는 마감 {s.nudges.length}건
        </Text>
        {#if upcoming.length === 0}
          <Empty>예정된 일이 없습니다. Lucia가 "내일 다시 확인할게요" 같은 약속을 하면 여기에 보입니다.</Empty>
        {:else}
          <Items>
            {#each upcoming.slice(0, 6) as r (r.key)}
              <Item title={r.title} icon={r.icon}>{r.kind} · <Time at={r.at} /></Item>
            {/each}
          </Items>
        {/if}
      </Section>

      <Section id="recent" title="최근 활동" icon="activity">
        {#snippet end()}<a href={pathOf('activity')}>모두 보기</a>{/snippet}
        {#if s.activity.length === 0}
          <Empty>아직 기록된 활동이 없습니다.</Empty>
        {:else}
          <Items>
            {#each s.activity.slice(0, 6) as a (a.id)}
              <Item title={a.ui.label} icon={a.ui.icon} tone={a.ui.tone}>
                {a.ui.detail}
                {#snippet end()}<Time at={a.at} />{/snippet}
              </Item>
            {/each}
          </Items>
        {/if}
      </Section>
      <!-- 권한 모드(결정 0040): 홈에서는 고르기만 하고, 모드마다 달라지는 것은 설정에서 본다. -->
      <Section id="security" title="권한 모드" icon="shield" hint={modeNow ? `지금: ${modeNow.label}` : undefined}>
        {#snippet lead()}엄격할 때와 널널해도 될 때를 골라 씁니다. Discord에서는 <code>/권한</code>으로도 바꿉니다.{/snippet}
        <Items>
          {#each consts.security.modes as m (m.key)}
            <Item icon={m.icon} tone={m.key === mode ? 'info' : 'muted'}>
              {#snippet heading()}
                {m.label}
                {#if m.key === mode}<Badge tone="info">지금</Badge>{/if}
              {/snippet}
              {m.summary}
              {#snippet end()}
                {#if m.key !== mode}
                  <PostButton action="/security/mode" fields={{ mode: m.key }}>{m.label}(으)로 바꾸기</PostButton>
                {/if}
              {/snippet}
            </Item>
          {/each}
        </Items>
        <Text tone="muted" size="sm" block>
          바꾸면 Discord로 알리고 활동 기록에 남습니다. 이미 떠 있는 카드에는 소급하지 않습니다.
        </Text>
        <p><a href={pathOf('safety')}>모드마다 달라지는 것 보기</a></p>
      </Section>

      <!-- 지금 실행: 루틴과 같은 작업을 큐에 넣는다. 각 줄에 마지막 결과를 보인다. -->
      <Section id="run-now" title="지금 실행" icon="play" hint="작업 큐에서 차례로 실행">
        <Items>
          <Item title="사전 체크">
            <p>캘린더·메일의 새 변화를 확인합니다. 모델을 쓰지 않습니다.</p>
            <LastRun run={s.last_precheck} />
            {#snippet end()}<PostButton action="/run/precheck" icon="refresh">확인</PostButton>{/snippet}
          </Item>
          <Item title="브리핑">
            <p>바뀐 것이 있으면 Discord로 브리핑을 보냅니다. 없으면 모델 없이 끝납니다.</p>
            <LastRun run={s.last_briefing} />
            {#snippet end()}<PostButton action="/run/briefing" icon="play">보내기</PostButton>{/snippet}
          </Item>
          <Item title="세션 정리(compact)">
            <p>상주 세션의 대화를 요약해 줄입니다. 보통은 자동으로 합니다.</p>
            <LastRun run={s.last_compact} />
            {#snippet end()}
              <Confirm
                action="/run/compact"
                label="정리"
                question="대화를 요약해 줄입니다(모델 1회). 실패하면 새 세션으로 넘어가고, 기억과 최근 대화로 이어 갑니다."
                confirmLabel="정리하기"
              />
            {/snippet}
          </Item>
          <Item title="세션 기록 백업">
            <p>VM 안의 대화 기록을 서버로 복사합니다. 매일 자동으로도 합니다.</p>
            <LastRun run={s.last_backup} />
            {#snippet end()}<PostButton action="/run/backup" icon="archive">백업</PostButton>{/snippet}
          </Item>
        </Items>
      </Section>

      <Section id="usage" title="사용량" icon="activity" hint="API 요금 환산">
        {#snippet end()}<a href={pathOf('system')}>자세히</a>{/snippet}
        <Grid min="xs" gap={3} stretch>
          <Stat label="오늘" value={usd(s.today.cost_usd)}>실행 {s.today.runs} · 모델 {s.today.model_runs}</Stat>
          <Stat label="최근 7일" value={usd(s.week.cost_usd)}>실행 {s.week.runs} · 모델 {s.week.model_runs}</Stat>
        </Grid>
        <Text tone="muted" size="sm" block>구독에서는 요금 대신 사용량 한도에서 차감됩니다.</Text>
      </Section>
  </Columns>
</Page>
