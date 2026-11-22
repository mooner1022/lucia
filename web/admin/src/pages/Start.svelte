<script lang="ts">
  import { usd } from '../lib/format';
  import type { PageData } from '../lib/types';
  import {
    ActionForm,
    Badge,
    Button,
    Callout,
    Cluster,
    Confirm,
    Disclosure,
    Empty,
    Item,
    Items,
    Jump,
    KV,
    KVRow,
    Page,
    Picture,
    PostButton,
    Section,
    SelectField,
    Text,
    TextArea,
    TextField,
    TONE_ICON,
  } from '../ui';

  // 초기 설정(결정 0064): 처음 쓰는 사람이 위에서 아래로 따라가는 여섯 단계. 필수는 대화할 곳 하나(Discord나 앱)뿐이고
  // 나머지는 선택이다. 단계의 상태·한 줄 요약·배지의 글은 데몬이 계산해 준다(`snapshot.setup`, derived.md 14장).
  // 이 화면의 폼은 연결 화면과 같은 처리기를 부른다(같은 주소, 같은 칸 이름). 처리기는 늘 연결 화면으로 돌려보내므로
  // `stay`로 이 화면에 남는다(안내는 그대로 보인다).
  let { data }: { data: PageData } = $props();
  const s = $derived(data.snapshot);
  const setup = $derived(s.setup);
  const consts = $derived(data.consts);
  const d = $derived(s.discord);
  const goog = $derived(s.google);
  const im = $derived(s.images);
  const profile = $derived(s.profile);

  // 화면 주소는 서버가 정한다(consts.pages). 모르는 키면 홈으로 둔다.
  const pathOf = (key: string) => consts.pages.find((p) => p.key === key)?.path ?? '/';

  // Discord 연결 상태: 멈췄으면 그 까닭.
  const stopped = $derived(typeof d.state === 'object' ? (d.state.Stopped?.reason ?? null) : null);

  // 알림 받을 곳 후보: 취소하지 않은 앱 토큰.
  const apps = $derived(s.clients.filter(([c]) => c.revoked_at === null && c.ui.is_app));
  // 앱이 하나라도 있으면 "앱 모두"를 고를 수 있다(값은 데몬이 준다, 결정 0066). 앱 하나를 고르는 것은 없다(결정 0067).
  const notifyOptions = $derived([
    { value: '', label: 'Discord DM' },
    ...(apps.length > 0 ? [{ value: consts.gateway.all_apps, label: '앱 모두(사람이 있는 기기에서 울림)' }] : []),
  ]);
  const gatewayOn = $derived(s.gateway_binds.length > 0);
</script>

{#snippet tokenForm(primary: boolean)}
  <ActionForm action="/discord/token" stay>
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
  <ActionForm action="/discord/owner" stay>
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
  <ActionForm action="/google/client/upload" multipart stay>
    <TextField label="클라이언트 JSON 파일" type="file" name="file" accept=".json,application/json" required>
      {#snippet action()}<Button type="submit" icon="upload">올리기</Button>{/snippet}
    </TextField>
  </ActionForm>
  <Disclosure kind="more" summary="파일 대신 내용 붙여 넣기">
    <ActionForm action="/google/client" stay>
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
{/snippet}

{#snippet pasteForm()}
  <ActionForm action="/google/paste" stay>
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

{#snippet avatarFoot()}
  {#if profile.avatar}
    <Cluster justify="end">
      <Confirm
        action="/profile/avatar/remove"
        label="그림 지우기"
        question="프로필 그림을 지울까요? 앱은 기본 모습으로 돌아가고, Discord 봇의 아바타는 데몬이 바꾼 적이 있으면 원래 것으로 돌아갑니다."
        confirmLabel="지우기"
        danger
        stay
      />
    </Cluster>
  {/if}
{/snippet}

<Page title="초기 설정" desc="처음 쓰는 사람을 위한 안내입니다. 필수는 대화할 곳 하나뿐이고, 나머지는 나중에 해도 됩니다.">
  {#if setup.complete}
    <Callout tone="ok" title="필수 설정을 마쳤습니다.">
      대화할 곳이 연결되어 있습니다. 아래 단계는 필요할 때 하면 됩니다.
    </Callout>
  {:else}
    <Callout tone="info" title="대화할 곳을 하나 연결하세요.">
      Discord(봇 토큰과 내 사용자 ID)나 앱(앱 토큰을 만들고 “알림 받을 곳”으로 고르기) 중 하나면 됩니다. 나머지는 모두 선택입니다.
    </Callout>
    <Cluster gap={3}>
      {#if setup.dismissed}
        <PostButton action="/start/reopen">홈 안내 다시 켜기</PostButton>
        <Text size="sm" tone="muted">지금은 홈에 초기 설정 안내가 보이지 않습니다.</Text>
      {:else}
        <PostButton action="/start/dismiss">나중에 하기</PostButton>
        <Text size="sm" tone="muted">홈에 보이는 초기 설정 안내를 끕니다. 이 화면은 메뉴에서 언제든 열 수 있습니다.</Text>
      {/if}
    </Cluster>
  {/if}

  <Jump items={setup.steps.map((step) => ({ id: step.key, label: step.title }))} />

  {#each setup.steps as step, i (step.key)}
    {#snippet end()}<Badge tone={step.tone}>{step.state_label}</Badge>{/snippet}
    {#snippet lead()}{step.summary}{/snippet}

    {#if step.key === 'profile'}
      <Section id={step.key} title="{i + 1}. {step.title}" icon={step.icon} {end} {lead} foot={profile.avatar ? avatarFoot : undefined}>
        <ActionForm action="/profile/name" stay>
          <TextField
            label="이름"
            help="관리 화면과 앱에 보이는 이름입니다. Lucia가 자기를 부르는 이름과 말투는 ‘캐릭터’에서 정하고, Discord에 보이는 이름은 Discord 개발자 포털에서 정합니다. 알림 글의 ‘Lucia’는 아직 그대로입니다."
            name="name"
            maxlength={consts.profile.name_max_chars}
            placeholder={consts.profile.default_name}
            value={profile.name_set ? profile.name : ''}
          >
            {#snippet action()}<Button type="submit">저장</Button>{/snippet}
          </TextField>
        </ActionForm>
        <Text size="sm" tone="muted" block>
          비우고 저장하면 기본 이름(“{consts.profile.default_name}”)으로 돌아갑니다. 이름은 {consts.profile.name_max_chars}자까지입니다.
          말투와 부르는 이름은 <a href={pathOf('character')}>캐릭터 화면</a>에서 정합니다.
        </Text>
        {#if profile.avatar}
          {@const avatar = profile.avatar}
          <Cluster gap={4} align="start">
            <Picture src={`/profile/avatar/file/${encodeURIComponent(avatar.id)}`} alt={`${data.name}의 프로필 그림`} size="lg" />
            <Text size="sm" tone="muted" block>{avatar.ui.facts} · {avatar.ui.by_label}</Text>
          </Cluster>
        {:else}
          <Empty>정한 프로필 그림이 없습니다. 앱에는 기본 모습이 보입니다.</Empty>
        {/if}
        <ActionForm action="/profile/avatar" multipart stay>
          <TextField
            label="프로필 그림"
            type="file"
            name="file"
            accept="image/png,image/jpeg"
            required
            help={`${consts.profile.avatar_rules}. 앱에 보이고, Discord가 연결되어 있으면 봇의 아바타도 바뀝니다(원래 아바타는 보관함에 남습니다).`}
          >
            {#snippet action()}<Button type="submit" icon="upload">올리기</Button>{/snippet}
          </TextField>
        </ActionForm>
      </Section>
    {:else if step.key === 'discord'}
      <Section id={step.key} title="{i + 1}. {step.title}" icon={step.icon} tone={stopped !== null ? 'bad' : undefined} {end} {lead}>
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
        <Disclosure summary="토큰과 사용자 ID는 어디서 얻나요">
          <ol>
            <li>
              <a href="https://discord.com/developers/applications" target="_blank" rel="noreferrer">Discord 개발자 포털</a>에서 새
              애플리케이션을 만듭니다.
            </li>
            <li>Bot 메뉴에서 <strong>Reset Token</strong>으로 봇 토큰을 만들어 복사합니다.</li>
            <li>같은 화면의 Privileged Gateway Intents에서 <strong>Message Content Intent</strong>를 켭니다.</li>
            <li>OAuth2 → URL Generator에서 <code>bot</code> 범위로 초대 주소를 만들어, 내가 있는 서버에 봇을 넣습니다.</li>
            <li>Discord 설정 → 고급에서 개발자 모드를 켜고, 내 프로필에서 사용자 ID를 복사합니다.</li>
          </ol>
        </Disclosure>
        <Text size="sm" tone="muted" block>
          시험 메시지, 다른 서버에서 부르기, 토큰·소유자 지우기는 <a href="{pathOf('connections')}#discord">연결 화면</a>에서 합니다.
        </Text>
      </Section>
    {:else if step.key === 'google'}
      <Section id={step.key} title="{i + 1}. {step.title}" icon={step.icon} tone={goog.ui.tone === 'bad' ? 'bad' : undefined} {end} {lead}>
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
          </KV>
          <Text size="sm" tone="muted" block>
            권한 추가, 클라이언트 JSON 바꾸기, 연결 끊기는 <a href="{pathOf('connections')}#google">연결 화면</a>에서 합니다.
          </Text>
        {:else}
          <Disclosure summary="클라이언트 JSON은 어디서 얻나요">
            <ol>
              <li>
                <a href="https://console.cloud.google.com/apis/credentials" target="_blank" rel="noreferrer">Google Cloud 콘솔</a>에서
                프로젝트를 만들고 <strong>Gmail API</strong>와 <strong>Google Calendar API</strong>를 켭니다.
              </li>
              <li>OAuth 동의 화면을 만들고, 사용자 유형이 외부이면 내 Google 계정을 테스트 사용자로 추가합니다.</li>
              <li>사용자 인증 정보에서 <strong>데스크톱 앱</strong> OAuth 클라이언트를 만들어 JSON을 내려받습니다.</li>
              <li>
                아래에 올리고 <strong>Google로 연결</strong>을 누릅니다. "확인되지 않은 앱"이 나오면 직접 만든 앱이라 정상입니다.
              </li>
              <li>동의가 끝나면 결과 화면이 열립니다. 거기서 “연결 화면으로”를 눌러 연결 상태를 확인하세요.</li>
            </ol>
          </Disclosure>
          {#if goog.has_client}
            <Text size="sm" block>클라이언트 JSON을 등록했습니다. 이제 Google 계정에 동의하세요.</Text>
            <Cluster>
              <PostButton action="/google/connect" icon="link" variant="primary" size="md" stay>Google로 연결</PostButton>
            </Cluster>
            <Disclosure kind="more" summary="다른 JSON으로 바꾸기">{@render clientForms()}</Disclosure>
          {:else}
            {@render clientForms()}
            <Cluster><Button icon="link" disabled>Google로 연결</Button></Cluster>
          {/if}
        {/if}
      </Section>
    {:else if step.key === 'app'}
      <Section id={step.key} title="{i + 1}. {step.title}" icon={step.icon} {end} {lead}>
        {#if !gatewayOn}
          <Callout tone="muted">
            접속 API가 꺼져 있습니다. 설정 파일의 <code>[gateway] bind</code>에 집 LAN·VPN 주소를 넣고 데몬을 다시 시작하세요. 이 화면은
            설정 파일을 고치지 않습니다.
          </Callout>
        {:else}
          <Text size="sm" block>
            주소:{' '}{#each s.gateway_binds as bind, n}{#if n > 0}{', '}{/if}<code>ws://{bind}/v1/ws</code>{/each}
          </Text>
          {#if apps.length > 0}
            <Items>
              {#each apps as [c, online] (c.id)}
                <Item title={c.name} icon="link" tone={online ? 'ok' : 'muted'}>
                  {#if online}연결됨{:else}연결 안 됨{/if}
                  {#if c.ui.is_notify_target}{' · '}<Badge tone="info">알림 받는 곳</Badge>{/if}
                </Item>
              {/each}
            </Items>
          {/if}
          <ActionForm action="/clients/new" stay>
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
          </ActionForm>
          <ActionForm action="/app/notify" stay>
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
            <ActionForm action="/app/fcm" stay>
              <TextArea
                label="Firebase 서비스 계정 JSON"
                help="Firebase 콘솔 → 프로젝트 설정 → 서비스 계정 → 새 비공개 키. 암호화해 저장하고 다시 보이지 않습니다."
                name="json"
                rows={4}
                required
              />
              <Cluster><Button type="submit" variant="primary">푸시 켜기</Button></Cluster>
            </ActionForm>
            <Disclosure summary="푸시(FCM)는 무엇이고 어디서 얻나요">
              <p>앱이 꺼져 있을 때 휴대폰을 깨우는 알림입니다. 없으면 앱이 꺼진 동안의 글은 Discord DM으로 갑니다.</p>
              <ol>
                <li>
                  <a href="https://console.firebase.google.com/" target="_blank" rel="noreferrer">Firebase 콘솔</a>에서 앱이 쓰는
                  프로젝트를 엽니다.
                </li>
                <li>프로젝트 설정 → 서비스 계정에서 <strong>새 비공개 키 생성</strong>을 눌러 JSON을 내려받습니다.</li>
                <li>그 JSON 내용을 위 칸에 붙여 넣고 <strong>푸시 켜기</strong>를 누릅니다.</li>
              </ol>
            </Disclosure>
          {/if}
          <Text size="sm" tone="muted" block>
            앱의 범위(권한), 토큰 취소, 시험 푸시, 푸시 미리보기는 <a href="{pathOf('connections')}#clients">연결 화면</a>에서 합니다.
          </Text>
        {/if}
      </Section>
    {:else if step.key === 'images'}
      <Section id={step.key} title="{i + 1}. {step.title}" icon={step.icon} {end} {lead}>
        {#if im.key_set}
          <KV>
            <KVRow label="API 키">
              <Cluster gap={2}><Badge tone="ok">저장됨</Badge><Text size="sm" tone="muted">값은 다시 보이지 않습니다</Text></Cluster>
            </KVRow>
          </KV>
          <Text size="sm" tone="muted" block>
            하루 예산과 키 바꾸기·지우기는 <a href="{pathOf('connections')}#images">연결 화면</a>에서 합니다.
          </Text>
        {:else}
          <ActionForm action="/images/key" stay>
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
          <Text size="sm" tone="muted" block>
            그림 만들기는 실제로 돈이 드는 기능입니다. 하루 예산은 기본 {usd(consts.images.default_daily_usd)}이고 연결 화면에서 바꿉니다.
          </Text>
        {/if}
      </Section>
    {:else if step.key === 'env'}
      <Section id={step.key} title="{i + 1}. {step.title}" icon={step.icon} {end} {lead}>
        <Text size="sm" tone="muted" block>
          이 화면은 샌드박스를 만들거나 설정 파일을 고치지 않습니다. 아래 명령은 Lucia 저장소 폴더의 터미널에서 직접 실행하세요.
        </Text>
        <Items>
          {#each setup.env as e (e.key)}
            <Item title={e.label} icon={TONE_ICON[e.tone]} tone={e.tone}>
              <p>{e.detail}</p>
              {#if e.command !== null}<p>명령: <code>{e.command}</code></p>{/if}
            </Item>
          {/each}
        </Items>
        <Text size="sm" tone="muted" block>점검 결과와 다시 점검은 <a href="{pathOf('system')}#checks">시스템 화면</a>에서 합니다.</Text>
      </Section>
    {/if}
  {/each}
</Page>
