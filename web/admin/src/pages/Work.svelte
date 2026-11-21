<script lang="ts">
  import type { PageData } from '../lib/types';
  import {
    Badge,
    Columns,
    Confirm,
    Disclosure,
    Empty,
    Item,
    Items,
    Jump,
    Page,
    Section,
    TONE_ICON,
    Time,
  } from '../ui';

  // 할 일: Lucia가 맡은 목표, 챙기는 마감, 작업, 약속한 후속 조치(결정 0010·0014·0032·0034).
  // 화면 설명의 차례(목표, 작업, 후속 조치, 마감)대로 적는다. 목표가 가장 길고 손댈 것이 많아 맨 앞이고,
  // 마감은 가장 짧아 맨 끝에 두어 두 단의 높이를 맞춘다.
  let { data }: { data: PageData } = $props();
  const s = $derived(data.snapshot);

  type GoalView = PageData['snapshot']['goals'][number];

  const openGoals = $derived(s.goals.filter((g) => g.ui.open).length);

  // 목표 한 줄에서 접어 둘 것(왜, 관심 신호, 하위 항목, 진행 기록)의 이름표. 안에 든 것만 적는다.
  function goalFoldLabel(v: GoalView): string | null {
    const parts: string[] = [];
    if (v.goal.purpose) parts.push('왜');
    if (v.goal.interests) parts.push('관심 신호');
    if (v.children.length > 0) parts.push(`하위 항목 ${v.children.length}개`);
    if (v.log.length > 0) parts.push(`진행 기록 ${v.log.length}건`);
    return parts.length > 0 ? parts.join(' · ') : null;
  }

  // 닫힌 시각과 결과는 데몬에서 Option이다(생성된 타입은 늘 있는 글로 적었다).
  const orNull = (value: string | null | undefined): string | null => value ?? null;

  // 결과 글은 앞 80자만(유니코드 글자 단위로 잘라 이모지가 깨지지 않게 한다).
  function clip(text: string | null | undefined): string | null {
    if (!text) return null;
    return [...text].slice(0, 80).join('');
  }
</script>

{#snippet goalFold(v: GoalView, label: string)}
  <Disclosure summary={label}>
    {#if v.goal.purpose}<p><strong>왜:</strong> {v.goal.purpose}</p>{/if}
    {#if v.goal.interests}<p><strong>관심 신호:</strong> {v.goal.interests.replace(/\n/g, ', ')}</p>{/if}
    {#if v.children.length > 0}
      <p>
        <strong>하위 항목:</strong>{' '}{#each v.children as c, i}{#if i > 0}{', '}{/if}{c.child}{#if c.by_review}{' (점검이 만듦)'}{/if}{/each}
      </p>
    {/if}
    {#if v.log.length > 0}
      <ul>
        {#each v.log as l}
          <li><Time at={l.at} />{' · '}{l.text}</li>
        {/each}
      </ul>
    {/if}
  </Disclosure>
{/snippet}

<Page title="할 일" desc="Lucia가 맡은 목표와 작업, 약속한 후속 조치, 챙기는 마감을 보고 손봅니다.">
  {#snippet tabs()}
    <Jump
      items={[
        { id: 'goals', label: '목표', count: openGoals },
        { id: 'tasks', label: '작업', count: s.tasks_open.length },
        { id: 'nudges', label: '챙기는 마감', count: s.nudges.length },
        { id: 'followups', label: '후속 조치', count: s.followups_open.length },
      ]}
    />
  {/snippet}

  <Columns>
    <!-- 맡은 목표(결정 0032): 상태, 마감, 진행 기록, 하위 항목, 지금 점검·완료·포기 -->
    <Section id="goals" title="맡은 목표" icon="tasks" hint={openGoals > 0 ? `${openGoals}개 진행 중` : undefined}>
      {#if s.goals.length === 0}
        <Empty>맡은 목표가 없습니다. "이번 달 안에 …까지 끝내자"처럼 맡기면 Lucia가 목표로 삼아 챙깁니다.</Empty>
      {:else}
        <Items>
          {#each s.goals as v (v.goal.id)}
            {@const g = v.goal}
            {@const open = v.ui.open}
            {@const fold = goalFoldLabel(v)}
            <Item icon="tasks" tone={open ? 'muted' : 'ok'}>
              {#snippet heading()}
                {g.title}
                <Badge tone={open ? 'info' : 'muted'}>{v.ui.status_label}</Badge>
              {/snippet}
              <p>
                끝 조건: {g.success}
                {#if g.deadline !== null}{' · 마감 '}<Time at={g.deadline} />{/if}
                {#if open && g.next_review_at !== null}{' · 다음 점검 '}<Time at={g.next_review_at} />{/if}
              </p>
              {#snippet end()}
                {#if open}
                  <Confirm
                    action={`/goals/${g.id}/review`}
                    label="지금 점검"
                    question="이 목표를 곧(1분 안) 점검합니다. 오늘 점검 상한에 닿았으면 내일로 미뤄집니다."
                    confirmLabel="점검하기"
                  />
                  <Confirm
                    action={`/goals/${g.id}/done`}
                    label="완료"
                    question="목표를 완료로 닫고 하위 후속 조치·루틴·감시를 끝냅니다. 사이트 위임은 그대로 둡니다."
                    confirmLabel="완료하기"
                  />
                  <Confirm
                    action={`/goals/${g.id}/abandon`}
                    label="포기"
                    question="목표를 포기로 닫고 하위 후속 조치·루틴·감시를 끝냅니다. 사이트 위임은 그대로 둡니다."
                    confirmLabel="포기하기"
                    danger
                  />
                {/if}
              {/snippet}
              {#snippet extra()}
                {#if fold !== null}{@render goalFold(v, fold)}{/if}
              {/snippet}
            </Item>
          {/each}
        </Items>
      {/if}
      <Disclosure summary="목표는 어떻게 챙기나">
        <p>
          Lucia가 점검 주기마다(그리고 관련 메일·하위 항목이 오면) 사람 없이 목표를 점검해 진행을 남기고 다음 할 일을 정합니다. 점검은 메일
          발송·폼 제출 없이 읽기와 로컬 쓰기만 합니다.
        </p>
      </Disclosure>
    </Section>

    <!-- 맡은 작업(결정 0014) -->
    <Section id="tasks" title="맡은 작업" icon="list" hint={s.tasks_open.length > 0 ? `${s.tasks_open.length}건 진행` : undefined}>
      {#if s.tasks_open.length === 0 && s.tasks_closed.length === 0}
        <Empty>맡은 작업이 없습니다. 오래 걸리는 일은 Lucia가 작업으로 받아 Discord 스레드에서 진행합니다.</Empty>
      {/if}
      {#if s.tasks_open.length > 0}
        <Items>
          {#each s.tasks_open as t (t.id)}
            {@const result = clip(t.result)}
            <Item icon="list" title={t.title}>
              <p>
                {t.ui.status_label}{' · '}<Time at={t.updated_at} />{' · '}{t.thread ? '작업 스레드' : 'DM'}{#if result !== null}{' · '}{result}{/if}
              </p>
              {#snippet end()}
                <Confirm
                  action={`/tasks/${t.id}/cancel`}
                  label="취소"
                  question="이 작업을 취소합니다. 하던 단계는 끝까지 가고 그 뒤 멈춥니다."
                  confirmLabel="취소하기"
                  danger
                />
              {/snippet}
            </Item>
          {/each}
        </Items>
      {/if}
      {#if s.tasks_closed.length > 0}
        <h3>최근에 끝난 작업</h3>
        <Items>
          {#each s.tasks_closed as t (t.id)}
            {@const closedAt = orNull(t.closed_at)}
            <Item icon={TONE_ICON[t.ui.tone]} tone={t.ui.tone} title={t.title}>
              {t.ui.status_label}{#if closedAt !== null}{' · '}<Time at={closedAt} />{/if}
            </Item>
          {/each}
        </Items>
      {/if}
    </Section>

    <!-- 챙기는 마감(결정 0034) -->
    <Section id="nudges" title="챙기는 마감" icon="flag" hint={s.nudges.length > 0 ? `${s.nudges.length}개` : undefined}>
      {#snippet lead()}처리되지 않으면 3일 전 소식, 하루 전 알림, 3시간 전 확인 카드로 다시 알립니다.{/snippet}
      {#if s.nudges.length === 0}
        <Empty>챙기는 마감이 없습니다. 메일·대화에서 마감이 나오면 Lucia가 여기에 올립니다.</Empty>
      {:else}
        <Items>
          {#each s.nudges as n (n.id)}
            <Item icon="flag" title={n.title}>
              <p>
                마감 <Time at={n.due_at} />{' · '}{n.ui.stage_label}{#if n.note}{' · '}{n.note}{/if}
              </p>
              {#snippet end()}
                <Confirm
                  action={`/nudges/${n.id}/done`}
                  label="처리함"
                  question="처리한 것으로 닫습니다. 더 알리지 않습니다."
                  confirmLabel="닫기"
                />
              {/snippet}
            </Item>
          {/each}
        </Items>
      {/if}
    </Section>
  </Columns>

  <!-- Lucia가 약속한 일(결정 0010): 열린 후속 조치(취소·브라우저 허용)와 최근에 닫힌 것.
       두 단 밖에 전체 너비로 둔다: 약속의 글이 길고 끝난 것까지 쌓여, 실제 데이터에서는 이 구역이 나머지 셋을 합친 것보다 몇 배 길다. -->
  <Section
    id="followups"
    title="약속한 후속 조치"
    icon="clock"
    hint={s.followups_open.length > 0 ? `${s.followups_open.length}건 예정` : undefined}
  >
    {#if s.followups_open.length === 0 && s.followups_closed.length === 0}
      <Empty>
        아직 약속한 후속 조치가 없습니다. Lucia가 "내일 다시 확인할게요" 같은 약속을 하면 여기에 남고, 그 시각에 스스로 챙깁니다.
      </Empty>
    {/if}
    {#if s.followups_open.length > 0}
      <Items>
        {#each s.followups_open as f (f.id)}
          <Item icon="clock" title={f.task}>
            <p>
              <Time at={f.due_at} />{' 예정 · '}<Time at={f.created_at} />{' 등록 · '}{f.ui.origin_label}
            </p>
            {#snippet end()}
              {#if !f.ui.uses_browser}
                <Confirm
                  action={`/followups/${f.id}/allow-browser`}
                  label="브라우저 허용"
                  question="브리핑이 등록한 메모라 지금은 위임한 사이트를 주소로 열어 읽기만 합니다. 허용하면 그 시각에 누르기·입력까지 할 수 있습니다(위임 밖 사이트는 여전히 카드로 묻고, 제출은 승인 카드)."
                  confirmLabel="허용하기"
                />
              {/if}
              <Confirm
                action={`/followups/${f.id}/cancel`}
                label="취소"
                question="이 후속 조치를 취소합니다. Lucia는 그 시각에 깨어나지 않습니다."
                confirmLabel="취소하기"
                danger
              />
            {/snippet}
          </Item>
        {/each}
      </Items>
    {/if}
    {#if s.followups_closed.length > 0}
      <h3>최근에 끝난 것</h3>
      <Items>
        {#each s.followups_closed as f (f.id)}
          {@const closedAt = orNull(f.closed_at)}
          {@const result = clip(f.result)}
          <Item icon={TONE_ICON[f.ui.tone]} tone={f.ui.tone} title={f.task}>
            {f.ui.status_label}{#if closedAt !== null}{' · '}<Time at={closedAt} />{/if}{#if result !== null}{' · '}{result}{/if}
          </Item>
        {/each}
      </Items>
    {/if}
  </Section>
</Page>
