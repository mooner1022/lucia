<script lang="ts">
  import { SETTINGS_TABS } from '../lib/nav';
  import type { PageData } from '../lib/types';
  import {
    ActionForm,
    Button,
    CheckField,
    Disclosure,
    Empty,
    FieldSet,
    Grid,
    Item,
    Items,
    Page,
    PostButton,
    Section,
    Tabs,
    Text,
    TextArea,
    TextField,
    Time,
  } from '../ui';

  // 설정 > 알림: 알림 카드에서 누른 반응이 남긴 규칙, 새 메일·일정을 어떻게 나눠 알릴지(결정 0031). 규칙(짧은 목록)이 먼저다.
  let { data }: { data: PageData } = $props();
  const s = $derived(data.snapshot);
  const consts = $derived(data.consts);
  const v = $derived(s.signals);

  const tabItems = $derived(SETTINGS_TABS.map((t) => ({ href: t.href, label: t.label, current: t.key === data.page })));
  // 화면 주소는 서버가 정한다(consts.pages). 모르는 키면 홈으로 둔다.
  const pathOf = (key: string) => consts.pages.find((p) => p.key === key)?.path ?? '/';
</script>

<Page title="설정" docTitle={data.title} desc="새 메일·일정을 어떻게 알릴지, 알림에 누른 반응을 어떻게 기억할지 정합니다.">
  {#snippet tabs()}<Tabs label="설정 구역" items={tabItems} />{/snippet}
  <!-- 규칙 목록은 짧고 폼 구역은 길다. 두 단으로 나누면 옆이 크게 비므로 단을 쓰지 않고, 목록을 먼저, 폼을 그 아래 전체 너비로 둔다. -->
  <Section id="rules" title="알림 규칙" icon="bell" hint="알림 카드에서 누른 반응">
    {#if s.notice_rules.length === 0}
      <Empty>아직 규칙이 없습니다. 알림 카드에서 "유용해요 / 브리핑에만 / 필요 없어요"를 누르면 그 보낸 사람에 대한 규칙이 여기에 쌓입니다.</Empty>
    {:else}
      <Items>
        {#each s.notice_rules as r (r.key)}
          {#snippet ruleHeading()}{r.label}{' '}<Text size="sm" tone="muted">{`<${r.key}>`}</Text>{/snippet}
          {#snippet ruleEnd()}<PostButton action="/signals/rules/delete" fields={{ key: r.key }}>지우기</PostButton>{/snippet}
          <Item heading={ruleHeading} end={ruleEnd}>
            {r.ui.rule_label}{' · '}<Time at={r.created_at} />
          </Item>
        {/each}
      </Items>
    {/if}
  </Section>

  <Section
    id="signals-settings"
    title="새 메일·일정 알림"
    icon="bell"
    hint={`오늘 분류 ${v.classify_used}/${v.classify_daily}번 · 깨우기 ${v.wake_used}/${v.wake_daily}번`}
  >
    {#snippet lead()}정한 간격마다 메일·일정에서 바뀐 것만 확인합니다(모델 없음). 지금 알릴 것만 Lucia가 확인해 알립니다.{/snippet}
    <ActionForm action="/signals/settings">
      <FieldSet legend="나눠 알리기">
        <CheckField
          name="enabled"
          label="새 메일·일정을 나눠 알리기"
          help="끄면 바뀐 것은 지금처럼 아침 브리핑에서만 다룹니다(확인은 계속합니다)."
          checked={v.enabled}
        />
      </FieldSet>
      <FieldSet legend="정해진 보고">
        <CheckField name="weekly" label="주간 자기 보고(일요일 19:00, 제안은 [진행] 카드)" checked={v.weekly} />
        <CheckField name="evening" label="저녁 미리보기(21:00, 내일 챙길 것)" checked={v.evening} />
        <CheckField
          name="consistency"
          label="메일·일정 맞춰 보기(12:30, 18:30)"
          help="일정 겹침과 빠듯한 이동 시간을 소식으로 알리고, 날짜·마감이 든 새 메일이 있으면 Lucia가 캘린더에 없는 것을 넣거나 묻습니다(참석자 없는 내 일정만 바로 넣습니다)."
          checked={v.consistency}
        />
      </FieldSet>
      <!-- 짧은 숫자 칸 셋은 한 줄에 나란히 둔다(좁으면 저절로 줄을 바꾼다). -->
      <Grid min="xs" gap={3}>
        <TextField
          label="확인 간격(분)"
          type="number"
          name="poll_minutes"
          min={consts.signals.poll_minutes_range[0]}
          max={consts.signals.poll_minutes_range[1]}
          value={v.poll_minutes}
          width="sm"
        />
        <TextField
          label="하루 분류 상한(번)"
          help="넘으면 모델 없이 모두 소식으로 모읍니다."
          type="number"
          name="classify_daily"
          min={consts.signals.daily_range[0]}
          max={consts.signals.daily_range[1]}
          value={v.classify_daily}
          width="sm"
        />
        <TextField
          label="하루 깨우기 상한(번)"
          type="number"
          name="wake_daily"
          min={consts.signals.daily_range[0]}
          max={consts.signals.daily_range[1]}
          value={v.wake_daily}
          width="sm"
        />
      </Grid>
      <TextArea
        label="관심 발신자·키워드(줄마다 하나)"
        help="보낸 사람·제목에 들어 있으면 관심으로 표시하고, 버리지 않습니다."
        name="interests"
        rows={4}
        placeholder="예: example.ac.kr
교수님 이름
과제"
        value={v.interests}
      />
      <Button type="submit">저장</Button>
    </ActionForm>
    <Disclosure summary="어떻게 나누나">
      <p>
        광고·소셜 메일과 내가 바꾼 일정은 거르고, 나머지는 작은 모델이 "지금 알릴 것 / 소식으로 모을 것 / 버릴 것"으로 나눕니다(제목·보낸 사람·짧은 미리보기만 봅니다). 지금 알릴 것만 Lucia가 확인해 알립니다(메일 발송·폼 제출 없이 읽기만).
      </p>
      <p>분류된 최근 신호는 <a href={pathOf('activity')}>활동</a>에서 봅니다.</p>
    </Disclosure>
  </Section>
</Page>
