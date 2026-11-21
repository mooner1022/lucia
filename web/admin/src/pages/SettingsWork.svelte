<script lang="ts">
  import { SETTINGS_TABS } from '../lib/nav';
  import type { PageData } from '../lib/types';
  import {
    ActionForm,
    Badge,
    Button,
    Columns,
    Confirm,
    Disclosure,
    Empty,
    Item,
    Items,
    Page,
    Section,
    Tabs,
    Text,
    TextArea,
    TextField,
    Time,
  } from '../ui';

  // 설정 > 작업과 보관: 파일 보관함(결정 0052), 하루 정리·보관(0016), 작업 스레드(0014), 개발 위임(0020).
  // 보관 파일은 줄 목록이라 폭을 다 쓰고, 그 아래에 설정 구역을 둔다.
  let { data }: { data: PageData } = $props();
  const s = $derived(data.snapshot);
  const consts = $derived(data.consts);

  const tabItems = $derived(SETTINGS_TABS.map((t) => ({ href: t.href, label: t.label, current: t.key === data.page })));
  // 화면 주소는 서버가 정한다(consts.pages). 모르는 키면 홈으로 둔다.
  const pathOf = (key: string) => consts.pages.find((p) => p.key === key)?.path ?? '/';
  // 비용 상한 칸: 비어 있으면 빈 글, 있으면 숫자 그대로.
  const money = (v: number | null): string => (v === null ? '' : String(v));
</script>

<Page title="설정" docTitle={data.title} desc="맡긴 작업을 어디서 진행할지, 개발 위임의 한도, 대화 원본을 얼마나 보관할지 정합니다.">
  {#snippet tabs()}<Tabs label="설정 구역" items={tabItems} />{/snippet}

  <Section id="kept-files" title="파일 보관함" icon="upload" hint={`${s.kept_files.length}개 · ${s.ui.kept_files_total_text}`}>
    {#snippet lead()}주고받은 파일과 만든 그림은 30일 뒤 지워집니다. 대화에서 "이거 보관해 둬"라고 하면 Lucia가 여기에 넣고, 여기 있는 파일은 지워지지 않습니다.{/snippet}
    {#if s.kept_files.length === 0}
      <Empty>보관한 파일이 없습니다.</Empty>
    {:else}
      <Items>
        {#each s.kept_files as k (k.file.id)}
          {#snippet fileHeading()}
            {#if k.title === ''}{k.file.name}{:else}{k.title}{' '}<Text size="sm" tone="muted">{k.file.name}</Text>{/if}
          {/snippet}
          {#snippet fileEnd()}
            <Button href={`/files/${k.file.id}/download`} download size="sm">내려받기</Button>
            <Confirm
              action={`/files/${k.file.id}/unkeep`}
              label="빼기"
              question="보관함에서 뺍니다. 받거나 만든 지 30일이 지난 파일은 다음 정리 때 지워집니다."
              confirmLabel="빼기"
              danger
            />
          {/snippet}
          <Item heading={fileHeading} icon="upload" end={fileEnd}>
            {#if k.note !== ''}{k.note}{' · '}{/if}{k.ui.kind_label}{' · '}{k.ui.size_text}{' · '}{k.ui.direction_label}{' · 보관 '}<Time at={k.kept_at} />
          </Item>
        {/each}
      </Items>
    {/if}
  </Section>

  <Columns>
    <Section id="wrapup" title="하루 정리·보관" icon="archive">
      {#snippet lead()}매일 20:30에 그날 대화를 읽고 오래 갈 것만 기억 항목에 반영합니다. 새 대화가 없으면 모델을 부르지 않습니다.{/snippet}
      <ActionForm action="/wrapup/settings">
        <TextField
          label="대화 원본 보관 기간(일)"
          help="그보다 오래된 대화 원본을 하루 정리 때 지웁니다. 지운 원본은 되돌릴 수 없고, 기억 항목과 세션 요약은 남습니다. 비워 두면 지우지 않습니다."
          type="number"
          name="keep_days"
          min={consts.wrapup.keep_days_range[0]}
          max={consts.wrapup.keep_days_range[1]}
          placeholder="비우면 지우지 않음"
          value={s.keep_days === null ? '' : String(s.keep_days)}
          width="sm"
        />
        <Button type="submit">저장</Button>
      </ActionForm>
      <Text size="sm" tone="muted" block>바뀐 기억은 <a href={pathOf('memory')}>기억</a> 화면의 변경 기록에서 보고 되돌릴 수 있습니다.</Text>
    </Section>

    <Section id="task-channel" title="작업 스레드" icon="list">
      {#snippet end()}
        {#if s.task_channel !== null}<Badge tone="ok">서버 채널</Badge>{:else}<Badge>DM</Badge>{/if}
      {/snippet}
      {#snippet lead()}Lucia에게 맡긴 작업마다 Discord 스레드 하나를 열어 진행과 결과를 모읍니다. 비워 두면 작업도 DM에서 합니다.{/snippet}
      <ActionForm action="/tasks/channel">
        <TextField
          label="채널 ID"
          help="DM에는 스레드가 없어, 서버의 텍스트 채널이 필요합니다."
          name="channel"
          inputmode="numeric"
          placeholder="예: 1234567890123456789"
          value={s.task_channel ?? ''}
          width="md"
        >
          {#snippet action()}<Button type="submit">저장</Button>{/snippet}
        </TextField>
      </ActionForm>
      <Disclosure summary="채널을 준비하는 방법">
        <ol>
          <li>Discord 개인 서버에 작업용 텍스트 채널을 만들고, 봇을 서버에 초대합니다(채널 보기, 메시지 보내기, 공개 스레드 만들기, 스레드에서 메시지 보내기 권한).</li>
          <li>Discord 개발자 포털 → 봇 → Privileged Gateway Intents에서 Message Content Intent를 켭니다. 꺼져 있으면 스레드에서 Lucia를 멘션한 말만 받습니다.</li>
          <li>Discord 설정의 개발자 모드를 켜고 채널을 우클릭 → ID 복사로 채널 ID를 넣습니다.</li>
        </ol>
      </Disclosure>
    </Section>

    <Section id="dev" title="개발 위임" icon="server">
      {#snippet lead()}Lucia에게 레포 수정을 맡기면 데몬이 레포를 클론해 작업마다 임시 샌드박스에서 작업 세션을 돌립니다. 결과는 lucia/{'<작업>'} 가지로, Discord 카드에서 승인해야 push합니다(강제 push 없음).{/snippet}
      <ActionForm action="/dev/settings">
        <TextField
          label={`작업 하나의 비용 상한(달러, API 환산, 비우면 ${consts.dev.default_cost_cap_usd})`}
          name="cost_cap"
          inputmode="decimal"
          value={money(s.dev_cost_cap)}
          width="sm"
        />
        <TextArea
          label="임시 샌드박스에서 열 주소(줄마다 호스트[:포트], 예: index.crates.io:443)"
          help="비우면 데몬 연결 말고는 막힙니다."
          name="allow_hosts"
          rows={3}
          value={s.dev_allow_hosts}
        />
        <TextArea
          label="맡길 수 있는 로컬 레포의 상위 경로(줄마다 절대 경로)"
          help="비우면 https 주소만 받습니다."
          name="local_roots"
          rows={2}
          value={s.dev_local_roots}
        />
        <Button type="submit">저장</Button>
      </ActionForm>
      <Text size="sm" tone="muted" block>작업이 끝나면 샌드박스와 클론을 지웁니다.</Text>
    </Section>
  </Columns>
</Page>
