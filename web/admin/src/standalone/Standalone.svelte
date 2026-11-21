<script lang="ts">
  import { load } from '../lib/app.svelte';
  import type { StandaloneData } from '../lib/types';
  import { ActionForm, Button, Callout, CheckField, Solo, Text, TextField } from '../ui';

  // 로그인 세션 없이 여는 화면: 로그인, 초기 설정, 사이트 로그인 입력(결정 0006·0017·0038), Google 동의 뒤 돌아옴.
  // 무엇을 그릴지는 서버가 정한다(`screen.type`). 틀린 비밀번호 같은 답도 같은 꼴로 와서 그대로 다시 그린다.
  let { data }: { data: StandaloneData } = $props();
  const s = $derived(data.screen);

  // 로그인하는 중이면 몇 초마다 다시 물어 결과를 보인다(예전의 자동 새로 고침).
  $effect(() => {
    if (s.type !== 'site_login_busy') return;
    const timer = setInterval(() => void load({ quiet: true }), 3000);
    return () => clearInterval(timer);
  });
</script>

{#if s.type === 'login'}
  <Solo title="로그인">
    {#if s.error}<Callout tone="bad" role="alert">{s.error}</Callout>{/if}
    <ActionForm action="/login" track={false}>
      <TextField label="관리 비밀번호" type="password" name="password" autocomplete="current-password" required autofocus />
      {#if s.external}
        <!-- 밖에서 들어오면 2단계 인증 코드도 받는다(결정 0043). -->
        <TextField
          label="인증 코드"
          help="인증 앱이 보여 주는 6자리 숫자"
          name="code"
          inputmode="numeric"
          autocomplete="one-time-code"
          pattern={'[0-9 ]{6,7}'}
          maxlength={7}
          required
        />
      {/if}
      <Button type="submit" variant="primary" block>로그인</Button>
    </ActionForm>
    <Text size="sm" tone="muted" block>
      {#if s.external}
        밖에서 로그인하면 Discord로 알립니다. 비밀번호를 잊었으면 서버에서 <code>luciad admin-setup --reset</code>
      {:else}
        집 네트워크(밖에서는 WireGuard나 공개 주소)에서 열립니다. 비밀번호를 잊었으면 서버에서
        <code>luciad admin-setup --reset</code>
      {/if}
    </Text>
  </Solo>
{:else if s.type === 'no_password'}
  <Solo title="처음 설정">
    <Callout tone="info">
      아직 관리 비밀번호가 없습니다. 서버 터미널에서 <code>luciad admin-setup</code>을 실행해 일회용 설정 링크를 받으세요.
    </Callout>
  </Solo>
{:else if s.type === 'setup'}
  <Solo title="처음 설정">
    <Text tone="subtle" block>관리 비밀번호를 정합니다({s.min_chars}자 이상).</Text>
    {#if s.error}<Callout tone="bad" role="alert">{s.error}</Callout>{/if}
    <ActionForm action="/setup" track={false}>
      <input type="hidden" name="token" value={s.token} />
      <TextField label="새 비밀번호" type="password" name="password" autocomplete="new-password" required minlength={s.min_chars} />
      <TextField label="한 번 더" type="password" name="confirm" autocomplete="new-password" required minlength={s.min_chars} />
      <Button type="submit" variant="primary" block>저장</Button>
    </ActionForm>
  </Solo>
{:else if s.type === 'setup_invalid'}
  <Solo title="처음 설정">
    <Callout tone="warn">
      설정 링크가 유효하지 않습니다. 이미 비밀번호가 있거나, 링크가 만료됐거나, 틀렸습니다. 서버에서
      <code>luciad admin-setup</code>으로 새 링크를 만드세요.
    </Callout>
  </Solo>
{:else if s.type === 'google_returned'}
  <Solo title="Google 연결">
    <Callout tone={s.ok ? 'ok' : 'bad'} role={s.ok ? 'status' : 'alert'}>{s.text}</Callout>
    <Button href="/connections" variant="primary" block>연결 화면으로</Button>
  </Solo>
{:else if s.type === 'site_login_form'}
  <Solo title="{s.site} 로그인">
    <Text tone="subtle" block>
      Lucia의 브라우저로 <strong>{s.site}</strong>에 로그인합니다. 입력한 값은 Lucia(모델)에게 가지 않고, 데몬이 브라우저 칸에 바로
      넣습니다.
    </Text>
    <Callout tone="info">
      <p>로그인할 주소:</p>
      <p><strong><code>{s.origin}</code></strong></p>
      <p>이 주소가 그 사이트가 맞는지 확인하세요. 닮은 주소(철자가 다른 도메인)면 넣지 마세요.</p>
    </Callout>
    {#if s.error}<Callout tone="bad" role="alert">{s.error}</Callout>{/if}
    <ActionForm action="/site-login/{s.token}" track={false}>
      <TextField label="아이디" name="username" autocomplete="username" required maxlength={100} autofocus />
      <TextField label="비밀번호" type="password" name="password" autocomplete="current-password" required maxlength={200} />
      <CheckField name="save" label="저장해 두기(다음부터는 Discord 승인만으로 다시 로그인합니다)" checked />
      <Button type="submit" variant="primary" block>로그인</Button>
    </ActionForm>
    <Text size="sm" tone="muted" block>
      로그인을 누른 뒤 1분쯤 걸릴 수 있습니다. 한 번만 누르세요. 이 링크는 {s.minutes}분 동안 쓸 수 있고, 로그인에 실패하면 같은
      링크로 {s.attempts}번까지 다시 넣을 수 있습니다(결과는 Discord로도 알려 드립니다).
    </Text>
  </Solo>
{:else if s.type === 'site_login_busy'}
  <Solo title="{s.site} 로그인">
    <Callout tone="warn" role="status">로그인하고 있습니다. 1분쯤 걸릴 수 있습니다.</Callout>
    <Text size="sm" tone="muted" block>
      이 화면은 몇 초마다 스스로 새로 고쳐 결과를 보입니다. 닫아도 로그인은 계속되고, 결과는 Discord로도 알려 드립니다.
    </Text>
  </Solo>
{:else if s.type === 'site_login_done'}
  <Solo title="{s.site} 로그인">
    <Callout tone="ok" role="status">{s.site}에 로그인했습니다. {s.note}</Callout>
    <Text size="sm" tone="muted" block>이 창은 닫아도 됩니다. Discord에서 하던 일을 다시 말씀해 주세요.</Text>
  </Solo>
{:else if s.type === 'site_login_pending'}
  <Solo title="{s.site} 로그인">
    <Callout tone="warn" role="status">아직 끝나지 않았습니다. {s.note}</Callout>
    {#if s.code_link}
      <Button href={s.code_link} variant="primary" block>
        {s.code_link.includes('/browser/handover') ? '브라우저 넘겨받기(관리 페이지 로그인 필요)' : '인증 코드 넣기'}
      </Button>
    {/if}
  </Solo>
{:else if s.type === 'site_code_form'}
  <Solo title="{s.site} 인증 코드">
    <Text tone="subtle" block>
      Lucia의 브라우저가 <strong>{s.site}</strong> 로그인의 인증 코드 단계에 있습니다. 코드는 Lucia(모델)에게 가지 않고, 데몬이
      브라우저 칸에 바로 넣습니다.
    </Text>
    <Callout tone="info">
      <p>사이트 주소:</p>
      <p><strong><code>{s.origin}</code></strong></p>
      <p>이 주소가 맞는지 확인하세요.</p>
    </Callout>
    {#if s.error}<Callout tone="bad" role="alert">{s.error}</Callout>{/if}
    <ActionForm action="/site-login/code/{s.token}" track={false}>
      <TextField label="인증 코드" name="code" inputmode="numeric" autocomplete="one-time-code" required maxlength={12} autofocus />
      <Button type="submit" variant="primary" block>넣기</Button>
    </ActionForm>
  </Solo>
{:else if s.type === 'site_login_failed'}
  <Solo title="{s.site} 로그인">
    <Callout tone="bad" role="alert">로그인하지 못했습니다: {s.reason}</Callout>
    <Text size="sm" tone="muted" block>필요하면 Lucia에게 다시 로그인을 부탁해 주세요.</Text>
  </Solo>
{:else if s.type === 'site_login_invalid'}
  <Solo title="사이트 로그인">
    <Callout tone="warn">
      이 입력 링크는 쓸 수 없습니다. 이미 썼거나, 만료됐거나, 틀렸습니다. Lucia에게 다시 요청하면 새 링크를 보냅니다.
    </Callout>
  </Solo>
{:else}
  <Solo title="찾을 수 없습니다">
    <Text tone="subtle" block>이 주소에는 화면이 없습니다.</Text>
    <Button href="/" variant="primary" block>홈으로</Button>
  </Solo>
{/if}
