<script lang="ts">
  import type { PageData } from '../lib/types';
  import type { Snapshot } from '../lib/types.gen';
  import {
    ActionForm,
    Badge,
    Button,
    Callout,
    CodeBlock,
    Confirm,
    Disclosure,
    Empty,
    Grid,
    Item,
    Items,
    Jump,
    Page,
    PostButton,
    Section,
    SelectField,
    Text,
    TextArea,
    TextField,
    Time,
  } from '../ui';

  // 자동화: 반복 루틴(결정 0025)과 페이지 감시(결정 0019·0036). 사람이 없을 때 스스로 도는 일이다.
  // 두 구역 다 길고 감시에는 고치는 폼이 딸려 있어, 옆 단으로 나누지 않고 화면 너비를 다 쓰게 위아래로 둔다.
  let { data }: { data: PageData } = $props();
  const s = $derived(data.snapshot);
  const watchConsts = $derived(data.consts.watch);

  type Routine = Snapshot['routines'][number];
  // watches[i]는 [감시, 원천 상태 | null] 짝이다. 생성된 타입은 이 짝과 같은 모양의 배열 한 가지를 더 섞어 적었다.
  type WatchPair = Extract<Snapshot['watches'][number], readonly [unknown, unknown]>;
  type Watch = WatchPair[0];
  // 원천 상태는 생성된 타입에서 unknown이다(시험 데이터에 비어 있기도 하다). 화면이 쓰는 칸만 좁힌다.
  type WatchState = { last_error: string | null; checked_at: string | null };

  const watches = $derived(s.watches as WatchPair[]);
  const stateOf = (pair: WatchPair): WatchState | null => pair[1] as WatchState | null;

  // 다음 실행 시각은 데몬에서 Option이다.
  const orNull = (value: string | null | undefined): string | null => value ?? null;
  // 넣을 말·뺄 말은 줄바꿈으로 저장돼 있어 쉼표로 이어 보인다.
  const words = (text: string): string => text.replace(/\n/g, ', ');

  const notifyOptions = $derived(watchConsts.notify.map((n) => ({ value: n.key, label: n.label })));
  const fetcherOptions = $derived(watchConsts.fetchers.map((f) => ({ value: f.key, label: f.label })));
</script>

<!-- 감시 하나를 고치는 폼: 간격은 모든 감시에, 나머지는 게시판 감시에만 있다. -->
{#snippet watchEdit(w: Watch)}
  <ActionForm action={`/watch-edit/${w.id}`}>
    <Grid min="sm" gap={3}>
      <TextField
        label={`간격(분, ${w.ui.interval_min}~${watchConsts.interval_max})`}
        type="number"
        name="interval"
        min={w.ui.interval_min}
        max={watchConsts.interval_max}
        value={w.interval_minutes}
        width="sm"
      />
      {#if w.ui.is_items}
        <SelectField label="알림" name="notify" options={notifyOptions} value={w.notify} />
        <SelectField
          label="읽는 방식"
          help="로그인 세션·브라우저는 사이트 위임이 있어야 합니다. 브라우저는 계속 켜 둡니다."
          name="fetcher"
          options={fetcherOptions}
          value={w.fetcher}
        />
      {/if}
    </Grid>
    {#if w.ui.is_items}
      <Grid min="md" gap={3}>
        <TextArea label="넣을 말(줄이나 쉼표마다, 비우면 모든 새 글)" name="include" rows={2} value={w.include_words} />
        <TextArea label="뺄 말" name="exclude" rows={2} value={w.exclude_words} />
      </Grid>
    {/if}
    <Button type="submit">저장</Button>
  </ActionForm>
{/snippet}

<!-- 루틴의 접어 둘 것: 스크립트(있을 때)와 최근 실행 -->
{#snippet routineFolds(v: Routine)}
  {@const r = v.routine}
  {#if r.script !== null}
    <Disclosure summary={`스크립트 · 도구: ${r.tools.length === 0 ? '없음' : r.tools.join(', ')}`}>
      <CodeBlock text={r.script} />
    </Disclosure>
  {/if}
  {#if v.runs.length > 0}
    <Disclosure summary={`최근 실행 ${v.runs.length}건`}>
      <Items>
        {#each v.runs as run}
          <Item icon={run.ok ? 'check-circle' : 'x-circle'} tone={run.ok ? 'ok' : 'bad'}>
            {#snippet heading()}<Time at={run.at} />{/snippet}
            {run.text}
            {#snippet end()}
              {#if !run.ok}<Badge tone="bad">실패</Badge>{/if}
              {#if run.woke}<Badge tone="info" plain>Lucia</Badge>{/if}
            {/snippet}
          </Item>
        {/each}
      </Items>
    </Disclosure>
  {/if}
{/snippet}

<Page title="자동화" desc="정해진 때나 변화가 있을 때 Lucia가 스스로 하는 일입니다. 반복 루틴과 페이지 감시를 켜고 끄고 고칩니다.">
  {#snippet tabs()}
    <Jump
      items={[
        { id: 'routines', label: '반복 루틴', count: s.routines.length },
        { id: 'watches', label: '페이지 감시', count: s.watches.length },
      ]}
    />
  {/snippet}

  <Section id="routines" title="반복 루틴" icon="repeat" hint={`${s.routines.length}개`}>
    {#snippet lead()}Lucia에게 반복해서 해 달라고 하면 만드는 루틴입니다. 스크립트가 있으면 모델 없이 그것부터 돌고, 판단이 필요할 때만 Lucia를 깨웁니다.{/snippet}
    {#if s.routines.length === 0}
      <Empty>루틴이 없습니다. “평일 아침마다 …”, “새 메일 오면 …”처럼 맡기면 Lucia가 만듭니다.</Empty>
    {:else}
      <Items>
        {#each s.routines as v (v.routine.id)}
          {@const r = v.routine}
          {@const next = orNull(v.next)}
          <Item icon="repeat">
            {#snippet heading()}
              {r.name}
              <Badge tone={v.ui.tone}>{v.ui.status_label}</Badge>
            {/snippet}
            <p>
              {v.ui.trigger_label}{#if next !== null}{' · 다음 '}<Time at={next} />{/if}{' · 실행 '}{r.runs}{'번, Lucia '}{r.wakes}{'번'}{#if r.failures > 0}{' · 이어진 실패 '}{r.failures}{/if}
            </p>
            <p>할 일: {r.task}</p>
            {#snippet end()}
              {#if v.ui.can_run}<PostButton action={`/routines/${r.id}/run`} icon="play">지금 실행</PostButton>{/if}
              {#if v.ui.can_pause}<PostButton action={`/routines/${r.id}/pause`}>멈추기</PostButton>{/if}
              {#if v.ui.resume_label !== null}<PostButton action={`/routines/${r.id}/resume`}>{v.ui.resume_label}</PostButton>{/if}
              <Confirm
                action={`/routines/${r.id}/remove`}
                label="지우기"
                question="이 루틴을 지웁니다. 되살릴 수 없습니다."
                confirmLabel="지우기"
                danger
              />
            {/snippet}
            {#snippet extra()}{@render routineFolds(v)}{/snippet}
          </Item>
        {/each}
      </Items>
    {/if}
    <Text size="sm" tone="muted" block>
      {#if s.scripts_sandbox !== null}
        스크립트 샌드박스: <code>{s.scripts_sandbox}</code>
      {:else}
        스크립트 샌드박스가 꺼져 있습니다(설정 [scripts]). 스크립트 없는 루틴만 돕니다.
      {/if}
    </Text>
    <Disclosure summary="루틴은 어떻게 도나">
      <p>
        스크립트는 네트워크 없는 샌드박스에서 읽기 도구만 씁니다. 알릴 것만 알리고, 판단이 필요할 때만 Lucia를 깨웁니다. 세 번 이어 실패하면
        멈추고 알립니다.
      </p>
    </Disclosure>
  </Section>

  <!-- 페이지 감시(결정 0019·0036) -->
  <Section id="watches" title="페이지 감시" icon="search" hint={`${s.watches.length}개`}>
    {#snippet lead()}Lucia가 제안하고 Discord 카드에서 승인한 페이지입니다. 데몬이 모델 없이 정해진 간격으로 확인합니다.{/snippet}
    {#if s.ui.browser_watches > 0}
      <Callout tone="info">브라우저 상시 켜짐: 브라우저로 읽는 감시 {s.ui.browser_watches}개가 켜져 있어 브라우저 샌드박스(메모리 약 2 GB)를 닫지 않습니다.</Callout>
    {/if}
    {#if watches.length === 0}
      <Empty>감시가 없습니다. 같은 페이지 확인을 되풀이해 맡기면 Lucia가 감시를 제안합니다.</Empty>
    {:else}
      <Items>
        {#each watches as pair (pair[0].id)}
          {@const w = pair[0]}
          {@const st = stateOf(pair)}
          <Item icon="search">
            {#snippet heading()}
              {w.what}
              <Badge tone={w.ui.tone}>{w.ui.status_label}</Badge>
            {/snippet}
            <p><code>{w.url}</code></p>
            <p>
              {w.interval_minutes}분마다{#if st !== null && st.checked_at !== null}{' · 마지막 확인 '}<Time at={st.checked_at} />{/if}{#if w.failures > 0}{' · 이어 실패 '}{w.failures}{'번'}{/if}
            </p>
            {#if w.ui.is_items}
              <p>게시판 <code>{w.item_pattern}</code>{' · '}{w.ui.notify_label}{' · '}{w.ui.via_label}</p>
              {#if w.include_words || w.exclude_words}
                <p>
                  {#if w.include_words}넣을 말: {words(w.include_words)}{/if}{#if w.include_words && w.exclude_words}{' · '}{/if}{#if w.exclude_words}뺄 말: {words(w.exclude_words)}{/if}
                </p>
              {/if}
            {:else if w.start_text || w.end_text}
              <p>구간 “{w.start_text}” ~ “{w.end_text}”</p>
            {/if}
            {#if st !== null && st.last_error !== null}
              <Text tone="bad" size="sm" block>오류: {st.last_error}</Text>
            {/if}
            {#snippet end()}
              {#if w.ui.can_pause}<PostButton action={`/watches/${w.id}/pause`}>멈추기</PostButton>{/if}
              {#if w.ui.can_resume}<PostButton action={`/watches/${w.id}/resume`}>다시 켜기</PostButton>{/if}
              <Confirm
                action={`/watches/${w.id}/remove`}
                label="지우기"
                question="이 감시를 지웁니다. 되살릴 수 없습니다."
                confirmLabel="지우기"
                danger
              />
            {/snippet}
            {#snippet extra()}
              {#if w.ui.editable}
                <Disclosure kind="more" summary="고치기">{@render watchEdit(w)}</Disclosure>
              {/if}
            {/snippet}
          </Item>
        {/each}
      </Items>
    {/if}
    <h3>새 글 알림 수</h3>
    <ActionForm action="/watch-settings">
      <TextField
        label="감시 하나가 하루에 바로 보내는 새 글 알림 수"
        help="넘으면 묶음 소식으로 보냅니다."
        type="number"
        name="now_daily"
        min={watchConsts.now_daily_range[0]}
        max={watchConsts.now_daily_range[1]}
        value={s.watch_now_daily}
        width="sm"
      >
        {#snippet action()}<Button type="submit">저장</Button>{/snippet}
      </TextField>
    </ActionForm>
    <Disclosure summary="감시는 어떻게 읽나">
      <p>
        로그인 게시판은 로그인된 브라우저의 세션(쿠키)을 빌려 HTTP로 읽거나, 브라우저 감시 탭에서 읽습니다(로그인이 풀리면 위임된 로그인으로 다시
        들어가고, 안 되면 멈춘 뒤 로그인되면 스스로 다시 켭니다).
      </p>
      <p>
        게시판 감시는 처음 보는 글을 찾아 키워드에 맞는 것만 알리고, 구간 감시는 정한 구간이 바뀌었을 때 알립니다(세 번 이어 바뀌면 잡음으로 보고
        멈춤).
      </p>
      <p>
        같은 사이트는 {watchConsts.host_gap_minutes}분에 한 번까지 가져오고, 실패하면 간격을 늘려 가며 다시 해 봅니다. 감시에 반복
        루틴(on_change)을 붙이면 감시는 알리지 않고 루틴의 스크립트가 정합니다.
      </p>
    </Disclosure>
  </Section>
</Page>
