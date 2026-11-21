<script lang="ts">
  import { usd } from '../lib/format';
  import { SETTINGS_TABS } from '../lib/nav';
  import type { PageData } from '../lib/types';
  import {
    ActionForm,
    Badge,
    Button,
    Callout,
    CheckField,
    Cluster,
    Columns,
    Disclosure,
    Item,
    Items,
    Page,
    PostButton,
    Section,
    SelectField,
    Stack,
    Tabs,
    Text,
    TextArea,
    TextField,
  } from '../ui';

  // 설정 > 안전과 한도: 권한 모드(모드마다 달라지는 것까지), 자동 실행 한도, 외부 접속과 2단계 인증,
  // 출구 검사, 일정 중 조용히. Lucia가 스스로 할 수 있는 범위(권한·한도)를 먼저, 바깥 접속과 검사 설정은 그 뒤에 둔다.
  let { data }: { data: PageData } = $props();
  const s = $derived(data.snapshot);
  const consts = $derived(data.consts);
  const a = $derived(s.access);

  const tabItems = $derived(SETTINGS_TABS.map((t) => ({ href: t.href, label: t.label, current: t.key === data.page })));

  // 권한 모드: 지금 모드는 consts.security.modes의 key와 맞춘다.
  const mode = $derived(s.ui.security_mode);
  const modeNow = $derived(consts.security.modes.find((m) => m.key === mode));
  const modeItems = (key: string, kind: 'items' | 'keeps'): string[] => consts.security.modes.find((m) => m.key === key)?.[kind] ?? [];

  // 인증 앱에 넣을 키와 그것을 여는 주소. 생성된 타입은 글 배열이라 둘로 풀어 쓴다.
  const pending = $derived<{ secret: string; uri: string } | null>(
    a.totp_pending === null ? null : { secret: a.totp_pending[0] ?? '', uri: a.totp_pending[1] ?? '' },
  );

  // 비용 상한 칸: 비어 있으면 빈 글, 있으면 숫자 그대로.
  const money = (v: number | null): string => (v === null ? '' : String(v));

  // 일정 중 조용히: 켜져 있으면 지금 읽어 둔 바쁜 일정 수를 도움말 끝에 붙인다.
  const quietHelp = $derived(
    `하루 종일 일정은 빼고, 그 사이의 소식·후속 조치·마감 알림을 일정이 끝난 뒤로 미룹니다. 오류와 "지금" 알림(급한 메일)은 그대로 보냅니다.${s.quiet_calendar ? ` 지금 읽어 둔 앞으로 하루의 바쁜 일정: ${s.quiet_busy}개.` : ''}`,
  );
</script>

{#snippet modeList(title: string, items: string[])}
  <Stack gap={2}>
    <h3>{title}</h3>
    <ul>
      {#each items as item}<li>{item}</li>{/each}
    </ul>
  </Stack>
{/snippet}

<Page title="설정" docTitle={data.title} desc="Lucia가 어디까지 스스로 할 수 있는지, 얼마나 쓸 수 있는지 정합니다.">
  {#snippet tabs()}<Tabs label="설정 구역" items={tabItems} />{/snippet}
  <Columns>
    <!-- 권한 모드(결정 0040): 모드마다 달라지는 것까지 보인다. 홈에는 고르기만 있다. -->
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
      <Disclosure summary="모드마다 무엇이 달라지나">
        {@render modeList('엄격 [제안]', modeItems('strict', 'items'))}
        {@render modeList('해제(사람이 대화 중인 실행만)', modeItems('open', 'items'))}
        {@render modeList('해제에서도 그대로 두는 것', modeItems('open', 'keeps'))}
      </Disclosure>
    </Section>

    <!-- 자동 실행 한도(결정 0010): 비용 상한과 방해 금지 시간. -->
    <Section id="limits" title="자동 실행 한도" icon="sliders">
      {#snippet lead()}데몬이 스스로 부르는 실행(아침 브리핑, 후속 조치, 반복 루틴)에 적용합니다. 사람이 보낸 채팅은 막지 않습니다.{/snippet}
      <ActionForm action="/autonomy/limits">
        <Cluster gap={3} align="start">
          <TextField
            label="하루 상한(달러)"
            help={`오늘 ${usd(s.today.cost_usd)} 사용. 비우면 끕니다.`}
            name="daily"
            inputmode="decimal"
            placeholder="예: 2"
            value={money(s.daily_cap)}
            width="sm"
          />
          <TextField
            label="주 상한(달러, 최근 7일)"
            help={`최근 7일 ${usd(s.week.cost_usd)} 사용. 비우면 끕니다.`}
            name="weekly"
            inputmode="decimal"
            placeholder="예: 10"
            value={money(s.weekly_cap)}
            width="sm"
          />
        </Cluster>
        <TextField
          label="방해 금지 시간(현지 시각)"
          help="이 시간의 후속 조치, 모아 보는 소식은 끝난 뒤로 미룹니다. 오류 알림은 바로 보냅니다."
          name="quiet"
          placeholder="예: 23:00-07:00"
          value={s.quiet_hours ?? ''}
          width="md"
        />
        <TextField
          label="반복 루틴이 하루에 Lucia를 깨우는 횟수"
          help="모든 루틴을 합친 횟수입니다. 넘으면 그날 남은 깨우기를 건너뜁니다. 스크립트만으로 끝나는 실행과 그 알림은 세지 않습니다."
          type="number"
          name="routine_wakes"
          min={consts.routines.daily_wakes_range[0]}
          max={consts.routines.daily_wakes_range[1]}
          placeholder={`비우면 ${consts.routines.daily_wakes_max}`}
          value={s.routine_wakes ?? ''}
          width="sm"
        />
        <Text size="sm" tone="muted" block>비용은 원장의 API 요금 환산 값입니다(구독에서는 사용량 한도에서 차감).</Text>
        <Button type="submit">저장</Button>
      </ActionForm>
    </Section>

    <!-- 외부 접속과 2단계 인증(결정 0043). 켜고 끄는 일은 집 네트워크의 세션에서만 열린다. -->
    <Section id="access" title="외부 접속과 2단계 인증" icon="lock">
      {#snippet end()}
        {#if a.totp_enabled}<Badge tone="ok">2단계 인증 켜짐</Badge>{:else}<Badge>2단계 인증 꺼짐</Badge>{/if}
      {/snippet}
      {#snippet lead()}공개 주소(리버스 프록시 뒤)로 밖에서 관리 페이지를 열 때는 비밀번호와 인증 앱의 6자리 코드를 함께 받습니다. 집 네트워크에서는 비밀번호만 받습니다.{/snippet}
      <Stack gap={1}>
        <Text size="sm" block>
          관리 페이지 공개 주소:{' '}{#if a.admin_url !== null}<code>{a.admin_url}</code>{:else}없음(집 네트워크에서만){/if}
        </Text>
        <Text size="sm" block>
          접속 API 공개 주소:{' '}{#if a.gateway_url !== null}<code>{a.gateway_url}</code>{:else}없음(집 네트워크·VPN에서만){/if}
        </Text>
      </Stack>
      {#if a.external_blocked}
        <Callout tone="warn">밖에서의 로그인 실패가 많아 외부 로그인을 한 시간 막아 두었습니다.</Callout>
      {/if}
      {#if a.admin_url !== null && !a.totp_enabled}
        <Callout tone="warn">공개 주소가 있지만 2단계 인증이 꺼져 있어 밖에서는 로그인할 수 없습니다.</Callout>
      {/if}
      {#if a.session_external}
        <Text size="sm" tone="muted" block>2단계 인증을 켜고 끄는 일은 집 네트워크에서만 할 수 있습니다.</Text>
      {:else if pending !== null}
        <ol>
          <li>인증 앱(Google Authenticator, 1Password 등)에 계정을 더하고 키를 넣습니다: <code>{pending.secret}</code></li>
          <li>휴대폰에서 보고 있다면 <a href={pending.uri}>인증 앱으로 열기</a>를 눌러도 됩니다.</li>
          <li>앱이 보여 주는 6자리 코드를 넣어 확인합니다.</li>
        </ol>
        <ActionForm action="/totp/confirm">
          <TextField
            label="인증 코드"
            help="확인되면 이 키는 다시 보이지 않습니다."
            name="code"
            inputmode="numeric"
            autocomplete="one-time-code"
            maxlength={7}
            required
            width="sm"
          >
            {#snippet action()}<Button type="submit" variant="primary">확인하고 켜기</Button>{/snippet}
          </TextField>
        </ActionForm>
      {:else if a.totp_enabled}
        <ActionForm action="/totp/disable">
          <TextField
            label="끄려면 지금 코드"
            help="끄면 밖에서는 로그인할 수 없습니다."
            name="code"
            inputmode="numeric"
            autocomplete="one-time-code"
            maxlength={7}
            required
            width="sm"
          >
            {#snippet action()}<Button type="submit" variant="ghost-danger">2단계 인증 끄기</Button>{/snippet}
          </TextField>
        </ActionForm>
        <Cluster><PostButton action="/totp/start">인증 앱 바꾸기(새 키)</PostButton></Cluster>
      {:else}
        <Cluster><PostButton action="/totp/start" variant="primary" size="md">2단계 인증 설정</PostButton></Cluster>
      {/if}
      <Disclosure summary="밖에서 열 때 지키는 것">
        <ul>
          <li>공개 주소는 설정 파일 <code>[public]</code>에서 정하고, 믿는 프록시를 거친 요청만 외부로 봅니다.</li>
          <li>외부 로그인: 비밀번호 + 인증 코드, 주소마다 5번 틀리면 5분 잠금, 밖에서의 실패가 한 시간에 20번이면 외부 로그인을 한 시간 막고 알림.</li>
          <li>밖에서 로그인하면 Discord로 알립니다. 쿠키는 Secure, HSTS를 보냅니다.</li>
          <li>초기 설정 링크와 2단계 인증 바꾸기는 집 네트워크에서만 열립니다.</li>
        </ul>
      </Disclosure>
    </Section>

    <!-- 출구 검사(결정 0029): 사용자 자료가 실린 주소·입력. -->
    <Section id="egress" title="출구 검사" icon="shield">
      {#snippet end()}
        {#if s.egress_block}<Badge tone="ok">막는 중</Badge>{:else}<Badge>기록만</Badge>{/if}
      {/snippet}
      {#snippet lead()}Lucia가 읽은 메일·일정·기억과 겹치는 글이나 개인정보가 주소나 입력칸으로 나가려 하면 잡습니다.{/snippet}
      <ActionForm action="/egress/mode">
        <SelectField
          label="검사에 걸리면"
          help="처음 일주일은 기록만 두고 활동 기록에서 오탐을 본 뒤 막기로 바꾸기를 권합니다."
          name="mode"
          options={[
            { value: 'log', label: '기록만(활동 기록에 남김)' },
            { value: 'block', label: '막기(열지 않거나 승인을 받음)' },
          ]}
          value={s.egress_block ? 'block' : 'log'}
          width="md"
        />
        <Button type="submit">저장</Button>
      </ActionForm>
      <Disclosure summary="무엇을 잡나">
        <p>Lucia가 읽은 메일·일정·기억과 겹치는 글이나 전화번호·주소 같은 개인정보가 주소나 입력칸으로 나가려 하면 잡습니다.</p>
      </Disclosure>
    </Section>

    <!-- 일정 기반 방해 금지(결정 0034). -->
    <Section id="quiet-calendar" title="일정 중 조용히" icon="calendar">
      {#snippet end()}
        {#if s.quiet_calendar}<Badge tone="ok">켜짐</Badge>{:else}<Badge>꺼짐</Badge>{/if}
      {/snippet}
      <ActionForm action="/quiet/calendar">
        <CheckField name="on" label="캘린더에 일정이 있는 동안도 조용히(수업·약속 중)" help={quietHelp} checked={s.quiet_calendar} />
        <TextArea
          label="조용히 하지 않을 일정 제목 단어(줄마다)"
          name="except"
          rows={2}
          placeholder="예: 점심
운동"
          value={s.quiet_except}
        />
        <Button type="submit">저장</Button>
      </ActionForm>
    </Section>
  </Columns>
</Page>
