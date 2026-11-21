<script lang="ts">
  import { usd } from '../lib/format';
  import type { PageData } from '../lib/types';
  import type { Consts, Snapshot } from '../lib/types.gen';
  import {
    ActionForm,
    Badge,
    Button,
    Callout,
    Cell,
    CheckField,
    Cluster,
    Columns,
    Confirm,
    Disclosure,
    Empty,
    Item,
    Items,
    KV,
    KVRow,
    Page,
    PostButton,
    Progress,
    Row,
    Section,
    SelectField,
    Stack,
    Table,
    Text,
    TextArea,
    TextField,
    Time,
  } from '../ui';

  // 연결: Discord(대화·브리핑·승인 카드), 다른 서버에서 부르기(결정 0024), 앱·기기(결정 0021)와 앱 알림, Google(메일·일정), 이미지 생성.
  // 같은 갈래(Discord 둘, 앱 둘)는 붙여 두고, 두 단으로 나뉠 때 높이가 비슷하게 맞는 차례로 적는다.
  // 상태에 맞는 단계만 먼저 보이고(처음엔 해야 할 일, 연결된 뒤엔 상태와 바꾸기), 어느 상태에서든 고칠 수단에 닿는다.
  let { data }: { data: PageData } = $props();
  const s = $derived(data.snapshot);
  const d = $derived(s.discord);
  // Discord 연결 상태는 "NotConfigured" 또는 { Connecting | Connected | Stopped: {...} } 꼴로 온다.
  const stopped = $derived(typeof d.state === 'object' ? (d.state.Stopped?.reason ?? null) : null);
  // 서버 목록: 아직 모름(null) · { Ok: [...] } · { Err: "까닭" }.
  const servers = $derived(d.servers?.Ok ?? null);
  const serversError = $derived(d.servers?.Err ?? null);

  // PageData 타입에는 상수(`consts`)가 아직 없어 여기서 좁힌다(데몬은 보낸다).
  const consts = $derived((data as PageData & { consts: Consts }).consts);
  const goog = $derived(s.google);
  const im = $derived(s.images);

  type Client = Snapshot['clients'][number][0];
  type Source = Snapshot['google']['sources'][number];
  type ImageRequest = Snapshot['images']['recent'][number];
  // 원천 상태: 생성된 타입은 세 칸을 늘 있는 글로 적었지만 데몬에서는 비어 있을 수 있다.
  type SourceState = {
    last_error: string | null;
    cursor: string | null;
    baseline_at: string | null;
    checked_at: string | null;
  };
  const sourceState = (src: Source): SourceState | null => src.state;

  // 알림 받을 곳 후보: 취소하지 않은 앱 토큰.
  const apps = $derived(s.clients.map(([c]: [Client, boolean]) => c).filter((c) => c.revoked_at === null && c.ui.is_app));
  const notifyOptions = $derived([
    { value: '', label: 'Discord DM' },
    ...apps.map((c) => ({ value: c.id, label: `앱: ${c.name}` })),
  ]);
  // 밖에서 닿는 주소: 끝의 /를 떼고 https를 wss로 바꾼다.
  const outsideWs = $derived(
    s.access.gateway_url === null ? null : `${s.access.gateway_url.replace(/\/+$/, '').replace('https://', 'wss://')}/v1/ws`,
  );

  // 이미지 요청 한 줄의 사양: 편집이면 넣은 그림 수, 모델, 품질, 크기, 투명 배경, 비용.
  function imageFacts(r: ImageRequest): string {
    return [
      ...(r.inputs > 0 ? [`편집(그림 ${r.inputs}장)`] : []),
      r.model,
      r.quality,
      r.size,
      ...(r.background === 'transparent' ? ['투명 배경'] : []),
      usd(r.cost_usd),
    ].join(' · ');
  }
</script>

{#snippet tokenForm(primary: boolean)}
  <ActionForm action="/discord/token">
    <TextField
      label="봇 토큰"
      help="Developer Portal → 내 앱 → Bot → Reset Token. 저장하면 바로 연결을 시험하고, 값은 암호화해 둡니다."
      type="password"
      name="token"
      autocomplete="off"
      required
      placeholder="Discord 개발자 포털의 Bot 토큰"
    >
      {#snippet action()}
        <Button type="submit" variant={primary ? 'primary' : 'default'}>저장하고 연결</Button>
      {/snippet}
    </TextField>
  </ActionForm>
{/snippet}

{#snippet ownerForm(primary: boolean)}
  <ActionForm action="/discord/owner">
    <TextField
      label="내 Discord 사용자 ID"
      help={'설정 → 고급 → 개발자 모드를 켜고 내 프로필에서 "ID 복사". 저장하면 확인 DM을 보냅니다. 봇과 같은 서버에 있어야 합니다.'}
      name="user_id"
      inputmode="numeric"
      required
      placeholder="숫자 ID"
      value={d.owner ?? ''}
    >
      {#snippet action()}
        <Button type="submit" variant={primary ? 'primary' : 'default'}>저장</Button>
      {/snippet}
    </TextField>
  </ActionForm>
{/snippet}

{#snippet clientForms()}
  <Stack gap={3}>
    <ActionForm action="/google/client/upload" multipart>
      <TextField label="클라이언트 JSON 파일" type="file" name="file" accept=".json,application/json" required>
        {#snippet action()}<Button type="submit" icon="upload">올리기</Button>{/snippet}
      </TextField>
    </ActionForm>
    <Disclosure kind="more" summary="파일 대신 내용 붙여 넣기">
      <ActionForm action="/google/client">
        <TextField
          label="클라이언트 JSON 내용"
          name="json"
          autocomplete="off"
          required
          placeholder={'{"installed": …} 전체'}
        >
          {#snippet action()}<Button type="submit">저장</Button>{/snippet}
        </TextField>
      </ActionForm>
    </Disclosure>
  </Stack>
{/snippet}

{#snippet pasteForm()}
  <ActionForm action="/google/paste">
    <TextField
      label="동의 뒤 주소창의 주소 전체"
      name="url"
      autocomplete="off"
      required
      placeholder={'http://127.0.0.1:…/?state=…&code=…'}
    >
      {#snippet action()}<Button type="submit" variant="primary">연결 마치기</Button>{/snippet}
    </TextField>
  </ActionForm>
{/snippet}

<!-- 원천 한 줄(사전 체크): 상태 배지 아래에 기준선과 마지막 확인 시각. -->
{#snippet sourceLine(src: Source)}
  {@const st = sourceState(src)}
  {#if st === null}
    <Text tone="muted">아직 확인 안 함</Text>
  {:else}
    {@const baseline = st.cursor !== null ? st.baseline_at : null}
    <Stack gap={1}>
      <Cluster gap={2}>
        {#if st.last_error !== null}
          <Badge tone="bad">오류</Badge>
          <Text size="sm">{st.last_error}</Text>
        {:else if st.cursor !== null}
          <Badge tone="ok">감지 중</Badge>
        {:else}
          <Badge>기준선 대기</Badge>
        {/if}
      </Cluster>
      {#if baseline !== null || st.checked_at !== null}
        <Text size="sm" tone="muted" block>
          {#if baseline !== null}기준선 <Time at={baseline} /> 이후{/if}
          {#if baseline !== null && st.checked_at !== null}{' · '}{/if}
          {#if st.checked_at !== null}마지막 확인 <Time at={st.checked_at} />{/if}
        </Text>
      {/if}
    </Stack>
  {/if}
{/snippet}

{#snippet googleFoot()}
  <Cluster justify="between">
    <PostButton action="/google/connect" icon="link">다시 연결(권한 추가)</PostButton>
    <Confirm
      action="/google/disconnect"
      label="연결 끊기"
      question="Google 토큰을 철회하고 지웁니다. 메일·일정 도구와 사전 체크가 멈춥니다."
      confirmLabel="연결 끊기"
      danger
    />
  </Cluster>
{/snippet}

{#snippet appFoot()}
  <Cluster justify="end">
    <Confirm
      action="/app/fcm/remove"
      label="자격 증명 지우기"
      question="FCM 자격 증명을 지울까요? 끊긴 앱으로 갈 글은 Discord로 갑니다."
      confirmLabel="지우기"
      danger
    />
  </Cluster>
{/snippet}

{#snippet imagesFoot()}
  <Cluster justify="end">
    <Confirm
      action="/images/key/remove"
      label="키 지우기"
      question="OpenAI API 키를 지울까요? 그림 만들기가 꺼집니다."
      confirmLabel="지우기"
      danger
    />
  </Cluster>
{/snippet}

<Page title="연결" desc="Discord와 Google 계정, 자체 앱을 연결하고 상태를 확인합니다. 브라우저는 브라우저 화면에서 다룹니다.">
  <Columns>
      <Section id="discord" title="Discord" icon="chat" hint="대화·브리핑·승인 카드" tone={stopped !== null ? 'bad' : undefined}>
        {#snippet end()}<Badge tone={d.ui.tone}>{d.ui.status}</Badge>{/snippet}
        {#if stopped !== null}
          <Callout tone="bad" title="연결이 멈췄습니다.">{stopped} 토큰을 확인해 다시 넣어 주세요.</Callout>
        {/if}
        <KV>
          <KVRow label="봇">{d.ui.bot ?? '—'}</KVRow>
          <KVRow label="봇 토큰">
            {#if d.token_set}<Badge tone="ok">설정됨</Badge>{:else}<Badge>없음</Badge>{/if}
          </KVRow>
          <KVRow label="소유자">
            {#if d.owner}<Text mono>{d.owner}</Text>{:else}<Badge tone="warn">없음</Badge>{/if}
          </KVRow>
          <KVRow label="알림 대화">
            {#if d.notify}<Text mono size="sm">{d.notify}</Text>{:else}<Text tone="muted">소유자를 정하면 생깁니다</Text>{/if}
          </KVRow>
        </KV>
        {#if !d.token_set || stopped !== null}
          {@render tokenForm(true)}
        {:else}
          <Disclosure kind="more" summary="봇 토큰 바꾸기">{@render tokenForm(false)}</Disclosure>
        {/if}
        {#if !d.owner}
          {#if d.token_set}
            <Callout tone="warn">소유자가 없으면 <strong>아무 메시지도 받지 않습니다.</strong> 내 사용자 ID를 넣어 주세요.</Callout>
          {/if}
          {@render ownerForm(d.token_set)}
        {:else}
          <Disclosure kind="more" summary="소유자 바꾸기">{@render ownerForm(false)}</Disclosure>
        {/if}
        {#snippet foot()}
          {#if d.token_set || d.owner}
            <Cluster justify="between">
              <Cluster>
                {#if d.token_set && d.notify}<PostButton action="/discord/test" icon="send">시험 메시지</PostButton>{/if}
              </Cluster>
              <Cluster>
                {#if d.owner}
                  <Confirm
                    action="/discord/owner/remove"
                    label="소유자 지우기"
                    question="소유자를 지우면 아무 메시지도 받지 않습니다."
                    confirmLabel="지우기"
                    danger
                  />
                {/if}
                {#if d.token_set}
                  <Confirm
                    action="/discord/token/remove"
                    label="토큰 지우기"
                    question="토큰을 지우고 Discord 연결을 끊습니다. 대화와 승인 카드가 멈춥니다."
                    danger
                  />
                {/if}
              </Cluster>
            </Cluster>
          {/if}
        {/snippet}
      </Section>

      <Section id="discord-servers" title="Discord 서버" icon="users" hint="다른 서버에서 부르기·서버 관리">
        {#snippet end()}
          {#if d.guild_calls}<Badge tone="ok">켜짐</Badge>{:else}<Badge>꺼짐</Badge>{/if}
        {/snippet}
        {#snippet lead()}봇이 들어간 서버에서 멘션하거나 머리말로 부르면 Lucia가 그 채널에서 답합니다.{/snippet}
        {#if d.content_refused}
          <Callout tone="warn">
            Discord가 메시지 내용 intent를 거절해 멘션한 말만 받고 있습니다(머리말·다른 서버 대화 읽기 안 됨). 개발자 포털 → 봇 →
            Privileged Gateway Intents에서 Message Content Intent를 켠 뒤 다시 저장하세요.
          </Callout>
        {/if}
        <ActionForm action="/discord/guild-calls">
          <CheckField name="calls" label="다른 서버에서 부르기" checked={d.guild_calls} />
          <TextField
            label="머리말"
            help="머리말과 대화 기록 읽기는 개발자 포털에서 Message Content Intent를 켜야 합니다. 멘션은 없이도 됩니다."
            name="prefix"
            maxlength={20}
            placeholder="예: !루시아 (비우면 멘션·답장으로만)"
            value={d.prefix ?? ''}
          >
            {#snippet action()}<Button type="submit">저장</Button>{/snippet}
          </TextField>
        </ActionForm>
        {#if serversError !== null}
          <Text tone="muted" size="sm" block>서버 목록을 읽지 못했습니다: {serversError}</Text>
        {:else if servers === null}
          <Text tone="muted" size="sm" block>Discord가 연결되면 봇이 들어간 서버가 여기에 보입니다.</Text>
        {:else if servers.length === 0}
          <Text tone="muted" size="sm" block>봇이 들어간 서버가 없습니다.</Text>
        {:else}
          <Table head={['서버', 'ID', '관리']} caption="봇이 들어간 서버">
            {#each servers as server (server.id)}
              <Row>
                <Cell main>{server.name}</Cell>
                <Cell><Text mono>{server.id}</Text></Cell>
                <Cell>{#if server.admin}<Badge tone="ok">가능</Badge>{:else}<Badge>권한 없음</Badge>{/if}</Cell>
              </Row>
            {/each}
          </Table>
        {/if}
        <Disclosure summary="다른 서버에서는 어떻게 동작하나">
          <p>
            다른 사람의 말은 받지 않습니다. 그 채널은 여러 사람이 보므로 Lucia는 기억·메일 같은 사적인 내용을 꺼내지 않고, 도구 사용
            알림도 올리지 않습니다.
          </p>
          <p>서버 관리(내보내기·차단·역할·채널 등)는 봇이 관리자 권한을 가진 서버에서만, DM의 승인 카드를 누른 뒤에 합니다.</p>
        </Disclosure>
      </Section>

      <Section id="clients" title="앱·기기 연결" icon="link">
        {#snippet lead()}자체 앱이 Discord처럼 Lucia와 대화하고 승인 카드를 누를 수 있습니다. 토큰마다 이름을 붙여 발급하고, 잃어버린 기기의 토큰은 취소합니다.{/snippet}
        {#if s.gateway_binds.length === 0}
          <Callout tone="muted">접속 API가 꺼져 있습니다. 설정 파일의 <code>[gateway] bind</code>에 집 LAN·VPN 주소를 넣고 데몬을 다시 시작하세요.</Callout>
        {:else}
          <!-- 쉼표 앞에 공백이 끼지 않게 블록 사이를 붙여 쓴다. -->
          <Text size="sm" block>
            주소:{' '}{#each s.gateway_binds as bind, i}{#if i > 0}{', '}{/if}<code>ws://{bind}/v1/ws</code>{/each}{#if outsideWs !== null}{', 밖에서 '}<code>{outsideWs}</code>{/if}
          </Text>
        {/if}
        {#if s.clients.length === 0}
          <Empty>발급한 토큰이 없습니다.</Empty>
        {:else}
          <Items>
            {#each s.clients as [c, online] (c.id)}
              {@const revoked = c.revoked_at !== null}
              {#snippet clientHeading()}
                {c.name}
                {#if revoked}<Badge>취소됨</Badge>{:else if online}<Badge tone="ok">연결됨</Badge>{:else}<Badge>연결 안 됨</Badge>{/if}
              {/snippet}
              {#snippet clientEnd()}
                {#if c.ui.is_app && s.app.fcm_ready && c.ui.has_push_token}
                  <PostButton action={`/app/push-test/${c.id}`}>시험 푸시</PostButton>
                {/if}
                <Confirm
                  action={`/clients/${c.id}/revoke`}
                  label="취소"
                  question="이 토큰을 취소할까요? 그 앱은 바로 끊기고 다시 연결하지 못합니다."
                  confirmLabel="토큰 취소"
                  danger
                />
              {/snippet}
              {#snippet clientScopes()}
                <Disclosure kind="more" summary="범위">
                  <ActionForm action={`/clients/${c.id}/scopes`}>
                    <CheckField
                      name="scope_security_mode"
                      label="권한 모드 바꾸기(/권한) 허용"
                      help="토큰이 새면 해제 모드로 바꿀 수 있습니다. 바꾸면 알림이 갑니다."
                      checked={c.ui.has_security_mode}
                    />
                    <CheckField
                      name="scope_app_tools"
                      label="Lucia가 이 앱의 기능 부르기 허용"
                      help="앱이 내준 기능(휴대폰 위치, 알림 읽기 등)을 Lucia가 필요할 때 부릅니다. 무엇을 내줄지는 앱이 정합니다."
                      checked={c.ui.has_app_tools}
                    />
                    <CheckField
                      name="scope_browser_handover"
                      label="브라우저 화면 넘겨받기 허용"
                      help="사이트가 사람 확인(캡차 등)을 띄울 때 이 앱에서 그 화면을 보고 풉니다. 토큰이 새면 로그인된 브라우저를 보고 조작할 수 있습니다."
                      checked={c.ui.has_browser_handover}
                    />
                    <Cluster><Button type="submit" size="sm">범위 저장</Button></Cluster>
                  </ActionForm>
                </Disclosure>
              {/snippet}
              <Item
                heading={clientHeading}
                icon="link"
                tone={online && !revoked ? 'ok' : 'muted'}
                end={revoked ? undefined : clientEnd}
                extra={!revoked && c.ui.is_app ? clientScopes : undefined}
              >
                <p>
                  {c.ui.role_label} · <code>{c.id}</code> · 발급 <Time at={c.created_at} />
                  {#if c.last_seen_at !== null}{' · 마지막 '}<Time at={c.last_seen_at} />{/if}
                </p>
                {#if !revoked && c.ui.is_app}
                  <p>
                    푸시 {#if c.push_platform !== null}등록({c.push_platform}){:else}없음{/if}
                    {#if s.app.notify_client === c.id}{' · '}<Badge tone="info">알림 받는 곳</Badge>{/if}
                  </p>
                {/if}
                {#if c.revoked_at !== null}
                  <p>취소함 <Time at={c.revoked_at} /></p>
                {:else if c.ui.is_app}
                  <p>범위: {#if c.scopes.length === 0}기본(대화·카드·기록·파일·푸시){:else}{c.ui.scope_labels.join(', ')}{/if}</p>
                {/if}
              </Item>
            {/each}
          </Items>
        {/if}
        <ActionForm action="/clients/new">
          <TextField
            label="새 앱 토큰"
            help="토큰은 발급 직후 한 번만 보입니다. 데몬은 해시만 남깁니다."
            name="name"
            maxlength={40}
            required
            placeholder="예: 휴대폰 앱"
          >
            {#snippet action()}<Button type="submit">발급</Button>{/snippet}
          </TextField>
          <CheckField name="scope_security_mode" label="권한 모드 바꾸기(/권한) 허용" help="기본은 꺼짐. 보기만은 범위 없이 됩니다." />
          <CheckField
            name="scope_app_tools"
            label="Lucia가 이 앱의 기능 부르기 허용"
            help="기본은 꺼짐. 앱이 내준 기능(위치 등)을 Lucia가 부릅니다."
          />
          <CheckField
            name="scope_browser_handover"
            label="브라우저 화면 넘겨받기 허용"
            help="기본은 꺼짐. 사람 확인(캡차 등) 화면을 이 앱에서 보고 풉니다."
          />
        </ActionForm>
        <Text size="sm" tone="muted" block>외부 기기 토큰은 아직 이 화면에서 발급하지 않습니다(기기는 이벤트만 보냅니다).</Text>
      </Section>

      <Section
        id="app-push"
        title="앱 알림과 푸시"
        icon="bell"
        hint={s.app.fcm_ready ? '푸시 켜짐' : '푸시 꺼짐'}
        foot={s.app.fcm_ready ? appFoot : undefined}
      >
        {#snippet lead()}브리핑·알림·승인 카드를 받을 곳을 정합니다. 앱이 꺼져 있을 때는 푸시(FCM)로 깨우고, 푸시가 없으면 Discord DM으로 보냅니다.{/snippet}
        <ActionForm action="/app/notify">
          <SelectField
            label="알림 받을 곳"
            help="앱을 고르면 브리핑·알림·승인 카드가 그 앱으로 갑니다. Discord DM에서 직접 한 대화의 답은 DM으로 옵니다."
            name="client"
            options={notifyOptions}
            value={s.app.notify_client ?? ''}
          >
            {#snippet action()}<Button type="submit">저장</Button>{/snippet}
          </SelectField>
        </ActionForm>
        {#if s.app.fcm_ready}
          <Text size="sm" block>Firebase 프로젝트: {#if s.app.fcm_project !== null}<code>{s.app.fcm_project}</code>{:else}?{/if}</Text>
        {:else}
          <ActionForm action="/app/fcm">
            <TextArea
              label="Firebase 서비스 계정 JSON"
              help="Firebase 콘솔 → 프로젝트 설정 → 서비스 계정 → 새 비공개 키. 암호화해 저장하고 다시 보이지 않습니다."
              name="json"
              rows={4}
              required
            />
            <Cluster><Button type="submit" variant="primary">푸시 켜기</Button></Cluster>
          </ActionForm>
        {/if}
        <ActionForm action="/app/push-preview">
          <CheckField
            name="preview"
            label="푸시에 글 앞부분 싣기"
            help={'끄면 "새 메시지가 있습니다" 같은 고정 문구만 갑니다. 켜면 글이 Google을 거칩니다.'}
            checked={s.app.push_preview}
          />
          <Cluster><Button type="submit" size="sm">저장</Button></Cluster>
        </ActionForm>
      </Section>

      <Section
        id="google"
        title="Google"
        icon="mail"
        hint="캘린더·메일"
        tone={goog.ui.tone === 'bad' ? 'bad' : undefined}
        foot={goog.connected ? googleFoot : undefined}
      >
        {#snippet end()}<Badge tone={goog.ui.tone}>{goog.ui.status}</Badge>{/snippet}
        {#if goog.consent_minutes_left !== null}
          <Callout tone="info" title={`동의를 기다리는 중입니다(${goog.consent_minutes_left}분 남음).`}>
            동의 뒤 브라우저에 "연결할 수 없음"이 뜨면 주소창의 주소 전체를 아래에 붙여 넣으세요.
          </Callout>
          {@render pasteForm()}
        {/if}
        {#if goog.connected}
          <KV>
            <KVRow label="계정">{goog.account ?? '확인 안 됨'}</KVRow>
            <KVRow label="권한">
              {#if goog.ui.scopes.length === 0}
                <Badge tone="warn">알 수 없음</Badge>
              {:else}
                <Cluster gap={1}>
                  {#each goog.ui.scopes as scope (scope.label)}
                    {#if scope.granted}<Badge tone="ok">{scope.label}</Badge>{:else}<Badge tone="bad">{scope.label} 없음</Badge>{/if}
                  {/each}
                </Cluster>
              {/if}
            </KVRow>
            {#each goog.sources as src}
              <KVRow label={`${src.label} 사전 체크`}>{@render sourceLine(src)}</KVRow>
            {/each}
          </KV>
          {#if !goog.ui.has_gmail_send}
            <Callout tone="warn">
              메일 발송 권한이 없습니다. <strong>다시 연결</strong>에서 허락하면 더해집니다. 같은 계정이면 사전 체크 기준점은 그대로입니다.
            </Callout>
          {:else if !goog.ui.has_calendar_write}
            <Callout tone="warn">
              캘린더 쓰기 권한이 없습니다(메일 속 마감을 캘린더에 넣기, 결정 0033). <strong>다시 연결</strong>에서 허락하면 더해집니다.
              읽기는 지금처럼 됩니다.
            </Callout>
          {/if}
          <Disclosure kind="more" summary="클라이언트 JSON 바꾸기">{@render clientForms()}</Disclosure>
        {:else}
          <!-- 연결 전: 세 단계. 첫 단계가 끝나야 두 번째 단추가 켜진다. -->
          <Items>
            <Item icon={goog.has_client ? 'check' : 'upload'} tone={goog.has_client ? 'ok' : 'info'}>
              {#snippet heading()}1. OAuth 클라이언트 등록{#if goog.has_client}<Badge tone="ok">완료</Badge>{/if}{/snippet}
              {#if goog.has_client}
                <p>등록했습니다.</p>
              {:else}
                <p>
                  Google Cloud 콘솔 → API 및 서비스 → 사용자 인증 정보에서 <strong>데스크톱 앱</strong> OAuth 클라이언트를 만들고 JSON을
                  내려받아 올리세요.
                </p>
              {/if}
              {#snippet extra()}
                {#if goog.has_client}
                  <Disclosure kind="more" summary="다른 JSON으로 바꾸기">{@render clientForms()}</Disclosure>
                {:else}
                  {@render clientForms()}
                {/if}
              {/snippet}
            </Item>
            <Item icon="link" tone={goog.has_client ? 'info' : 'muted'}>
              {#snippet heading()}2. Google 계정 동의{/snippet}
              <p>Google 동의 화면으로 갑니다. "확인되지 않은 앱"이 나오면 직접 만든 앱이라 정상입니다.</p>
              {#snippet end()}
                {#if goog.has_client}
                  <PostButton action="/google/connect" icon="link" variant="primary" size="md">Google로 연결</PostButton>
                {:else}
                  <Button icon="link" disabled>Google로 연결</Button>
                {/if}
              {/snippet}
            </Item>
            <Item icon="info" tone="muted" extra={goog.consent_minutes_left === null ? pasteForm : undefined}>
              {#snippet heading()}3. 주소 붙여 넣기(필요할 때만){/snippet}
              <p>LAN 주소로 열었다면 동의 뒤 "연결할 수 없음"이 뜹니다. 그 주소창의 주소 전체를 10분 안에 붙여 넣으세요.</p>
            </Item>
          </Items>
        {/if}
      </Section>

      <Section id="images" title="이미지 생성" icon="spark" hint={im.key_set ? '켜짐' : '꺼짐'} foot={im.key_set ? imagesFoot : undefined}>
        {#snippet lead()}Lucia가 OpenAI의 이미지 모델(GPT Image 2.5 Flare·Sunburst)로 그림을 만들고 고칩니다. 실제로 돈이 드는 기능이라 Claude 사용량 상한과 따로 하루 예산을 둡니다.{/snippet}
        {#if im.key_set}
          <KV>
            <KVRow label="API 키">
              <Cluster gap={2}><Badge tone="ok">저장됨</Badge><Text size="sm" tone="muted">값은 다시 보이지 않습니다</Text></Cluster>
            </KVRow>
            <KVRow label="오늘 쓴 돈">
              <Progress value={im.today_usd} max={im.daily_cap} label={`${usd(im.today_usd)} / ${usd(im.daily_cap)}`} />
            </KVRow>
          </KV>
        {:else}
          <ActionForm action="/images/key">
            <TextField
              label="OpenAI API 키"
              help="OpenAI 콘솔에서 만든 키를 넣습니다. 암호화해 저장하고 다시 보이지 않습니다. 이미지 모델을 쓰려면 콘솔에서 조직 인증이 필요할 수 있습니다."
              type="password"
              name="key"
              required
              autocomplete="off"
            >
              {#snippet action()}<Button type="submit">저장</Button>{/snippet}
            </TextField>
          </ActionForm>
        {/if}
        <ActionForm action="/images/budget">
          <TextField
            label="하루 예산(달러)"
            help={`오늘 쓴 돈이 이 값에 닿으면 그날은 더 만들지 않습니다. 비우면 기본값(${usd(consts.images.default_daily_usd)}), 0이면 만들지 않습니다.`}
            name="daily"
            inputmode="decimal"
            value={String(im.daily_cap)}
            width="md"
          >
            {#snippet action()}<Button type="submit">저장</Button>{/snippet}
          </TextField>
        </ActionForm>
        {#if im.recent.length > 0}
          <h3>최근 요청</h3>
          <Items>
            {#each im.recent as r (r.id)}
              {#snippet requestHeading()}<Badge tone={r.ui.tone}>{r.ui.label}</Badge> {r.prompt}{/snippet}
              <Item heading={requestHeading}>
                <p>{imageFacts(r)} · <Time at={r.created_at} /></p>
                {#if r.error !== null}<p>{r.error}</p>{/if}
              </Item>
            {/each}
          </Items>
        {/if}
        <Disclosure summary="모델은 어떻게 고르나">
          <p>
            Lucia가 요청마다 고릅니다. Flare는 가장 빠른 일상용이고 기본입니다. Sunburst는 가장 정밀한 모델로, 세밀한 묘사나 그림 속
            글자처럼 정밀함이 중요할 때 씁니다. 두 모델의 요율은 같고, 비용은 품질과 크기에 따라 달라집니다.
          </p>
          <p>
            그림을 주고 고쳐 달라고 하면 그 그림을 바탕으로 만듭니다(편집). 이때는 고르지 않으면 Sunburst를 씁니다. 프롬프트와 넣은 그림은
            OpenAI로 그대로 나갑니다.
          </p>
        </Disclosure>
      </Section>
  </Columns>
</Page>
