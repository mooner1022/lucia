<script lang="ts">
  import { bytes, num, usd } from '../lib/format';
  import type { PageData, Tone } from '../lib/types';
  import type { Snapshot } from '../lib/types.gen';
  import {
    Badge,
    Cell,
    Cluster,
    Confirm,
    Columns,
    Empty,
    Item,
    Items,
    KV,
    KVRow,
    LastRun,
    Page,
    PostButton,
    Progress,
    Row,
    Section,
    Stack,
    Table,
    TONE_ICON,
    Text,
    Time,
  } from '../ui';

  // 시스템: 먼저 실행 환경 점검(문제가 있으면 구역이 붉게 보인다), 상주 세션의 맥락과 기억, 백업, 사용량,
  // 마지막으로 정기 루틴 표. 보기 위주이고 설정은 설정 화면에 있다. 지금 실행(정리, 백업, 다시 점검)만 여기서 누른다.
  let { data }: { data: PageData } = $props();
  const s = $derived(data.snapshot);

  // 화면 주소는 서버가 정한다(consts.pages). 모르는 키면 홈으로 둔다.
  const pathOf = (key: string) => data.consts.pages.find((p) => p.key === key)?.path ?? '/';

  type Schedule = Snapshot['schedules'][number];

  // 시험 데이터에서 늘 채워져 있던 칸이라 생성된 타입은 비어 있음을 적지 않았다(데몬에서는 Option이다).
  const asTime = (v: unknown): string | null => (typeof v === 'string' ? v : null);
  // 주기를 말로 풀지 못하면(null) 원문을 적는다.
  const cronOf = (text: string | null, raw: string): string => text ?? raw;
  const scheduleCron = (r: Schedule): string => cronOf(r.ui.cron_text as string | null, r.spec);

  const failed = $derived(s.checks.filter((c) => !c.ok).length);
  const checksHint = $derived(s.checks.length === 0 ? undefined : failed === 0 ? '모두 정상' : `${failed}개 실패`);

  // 대화 맥락: 자동 정리 기준(compact_at_tokens)이 있으면 그것을, 없으면 세션 교대 기준을 한도로 본다.
  const limit = $derived(s.compact_at_tokens ?? s.rotate_tokens);
  // 한도의 60%까지는 여유, 90%까지는 주의, 넘으면 위험(예전 계기판의 낮음·높음 경계와 같다).
  function contextTone(used: number, max: number): Tone {
    if (used <= Math.floor((max * 6) / 10)) return 'ok';
    return used <= Math.floor((max * 9) / 10) ? 'warn' : 'bad';
  }
</script>

{#snippet usageRow(name: string, today: string, week: string, faint: boolean)}
  <Row>
    <Cell main>{name}</Cell>
    <Cell num>{#if faint}<Text tone="muted">{today}</Text>{:else}{today}{/if}</Cell>
    <Cell num>{#if faint}<Text tone="muted">{week}</Text>{:else}{week}{/if}</Cell>
  </Row>
{/snippet}

{#snippet sessionFoot()}
  <Cluster justify="between">
    <Confirm
      action="/run/compact"
      label="지금 정리"
      question="대화를 요약해 줄입니다(모델 1회). 실패하면 새 세션으로 넘어가고, 기억과 최근 대화로 이어 갑니다."
      confirmLabel="정리하기"
    />
    <Text size="sm" tone="muted">기억 항목은 기억 화면에서 봅니다.</Text>
  </Cluster>
{/snippet}

<Page title="시스템" desc="실행 환경 점검, 상주 세션과 기억, 백업, 정기 루틴, 사용량을 봅니다.">
  <Columns>
    <Section id="checks" title="실행 환경 점검" icon="server" hint={checksHint} tone={failed > 0 ? 'bad' : 'ok'}>
      {#snippet end()}<PostButton action="/checks/refresh" icon="refresh">다시 점검</PostButton>{/snippet}
      {#if s.checks.length === 0}
        <Empty>아직 점검하지 않았습니다.</Empty>
      {:else}
        <Items>
          {#each s.checks as c}
            <Item title={c.name} icon={TONE_ICON[c.ok ? 'ok' : 'bad']} tone={c.ok ? 'ok' : 'bad'}>{c.detail}</Item>
          {/each}
        </Items>
        {#if s.checks_at !== null}
          <Text size="sm" tone="muted" block>마지막 점검 <Time at={s.checks_at} /> · 10분마다 자동으로 다시 합니다.</Text>
        {/if}
      {/if}
    </Section>

    <Section id="session" title="세션과 기억" icon="memory" foot={sessionFoot}>
      {#if s.session}
        {@const sess = s.session}
        {@const lastUsed = asTime(sess.last_used_at)}
        {@const lastCompacted = asTime(sess.last_compacted_at)}
        <!-- 계기판의 이름표가 곧 접근성 이름이라 무엇의 사용량인지 글에 같이 적는다. -->
        <Stack gap={1}>
          <Progress
            value={Math.min(sess.last_context_tokens, limit)}
            max={limit}
            label={`대화 맥락 사용량: ${num(sess.last_context_tokens)} / ${num(limit)} 토큰`}
            tone={contextTone(sess.last_context_tokens, limit)}
          />
          <Text size="sm" tone="muted" block>
            {#if s.compact_at_tokens !== null}
              이만큼 차면 자동으로 정리(compact)합니다. 한동안 조용해도 정리합니다.
            {:else}
              이만큼 차면 새 세션으로 교대합니다.
            {/if}
          </Text>
        </Stack>
        <KV>
          <KVRow label="상주 세션"><Text mono>{sess.ui.short_id}…</Text></KVRow>
          <KVRow label="마지막 대화">{#if lastUsed !== null}<Time at={lastUsed} />{:else}—{/if}</KVRow>
          <KVRow label="마지막 정리">
            {#if lastCompacted !== null}<Time at={lastCompacted} />{:else}<Text tone="muted">아직 없음</Text>{/if}
          </KVRow>
        </KV>
      {:else}
        <Empty>상주 세션이 아직 없습니다. 첫 대화에서 만들어집니다.</Empty>
      {/if}
      <KV>
        <KVRow label="기억 항목">
          {s.memories.length}개{' · '}{bytes(s.ui.memory_bytes)}{' · '}<a href={pathOf('memory')}>기억 보기</a>
        </KVRow>
      </KV>
    </Section>

    <Section id="backup" title="세션 기록 백업" icon="archive">
      {#snippet end()}<PostButton action="/run/backup" icon="archive">지금 백업</PostButton>{/snippet}
      <!-- 이 데몬이 한 마지막 백업이 있으면 그 결과를, 없으면 저장소에 남은 마지막 기록을 보인다. -->
      {#if s.last_backup}<LastRun run={s.last_backup} />{/if}
      <KV>
        {#if !s.last_backup}
          <KVRow label="마지막">
            {#if s.backup.last !== null}{s.backup.last}{:else}<Text tone="muted">아직 없음</Text>{/if}
          </KVRow>
        {/if}
        <KVRow label="주기">
          {#if s.backup.enabled}
            {cronOf(s.backup.ui.cron_text as string | null, s.backup.cron)}{' · '}{s.backup.keep_days}일 보관
          {:else}
            <Badge tone="muted">꺼짐</Badge>
          {/if}
        </KVRow>
        {#if s.last_backup && s.backup.last !== null}
          <KVRow label="저장된 것">{s.backup.last}</KVRow>
        {/if}
      </KV>
    </Section>

    <Section id="usage-table" title="사용량" icon="activity" hint="API 요금 환산">
      <Table head={['항목', '오늘', '최근 7일']} num={[1, 2]} caption="오늘과 최근 7일의 사용량">
        {@render usageRow('실행', String(s.today.runs), String(s.week.runs), false)}
        {@render usageRow('모델 실행', String(s.today.model_runs), String(s.week.model_runs), false)}
        {@render usageRow(
          '입력 토큰(캐시 포함)',
          num(s.today.input_tokens + s.today.cache_tokens),
          num(s.week.input_tokens + s.week.cache_tokens),
          false,
        )}
        {@render usageRow('그중 캐시', num(s.today.cache_tokens), num(s.week.cache_tokens), true)}
        {@render usageRow('출력 토큰', num(s.today.output_tokens), num(s.week.output_tokens), false)}
        {@render usageRow('비용', usd(s.today.cost_usd), usd(s.week.cost_usd), false)}
      </Table>
      <Text size="sm" tone="muted" block>
        모델 없이 끝난 실행(바뀐 게 없는 브리핑, 사전 체크)도 실행 수에 들어갑니다. 구독에서는 요금 대신 사용량 한도에서 차감됩니다.
      </Text>
    </Section>
  </Columns>

  <Section id="schedules" title="정기 루틴" icon="clock" hint="데몬이 정해진 때에 도는 일">
    {#if s.schedules.length === 0}
      <Empty>켜진 루틴이 없습니다.</Empty>
    {:else}
      <Table head={['이름', '주기', '다음', '마지막']} caption="정기 루틴">
        {#each s.schedules as r (r.id)}
          {@const next = asTime(r.next_due_at)}
          {@const last = asTime(r.last_fired_at)}
          <Row>
            <Cell main>{r.ui.label}</Cell>
            <Cell>{scheduleCron(r)}</Cell>
            <Cell>{#if next !== null}<Time at={next} />{:else}—{/if}</Cell>
            <Cell>{#if last !== null}<Time at={last} />{:else}<Text tone="muted">—</Text>{/if}</Cell>
          </Row>
        {/each}
      </Table>
    {/if}
    <Text size="sm" tone="muted" block>사용자가 만든 반복 루틴은 <a href={pathOf('automation')}>자동화</a>에서 다룹니다.</Text>
  </Section>
</Page>
