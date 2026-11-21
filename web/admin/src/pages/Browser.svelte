<script lang="ts">
  import type { PageData } from '../lib/types';
  import type { Snapshot } from '../lib/types.gen';
  import {
    ActionForm,
    Badge,
    Button,
    Callout,
    CheckField,
    Columns,
    Confirm,
    Disclosure,
    Empty,
    FieldSet,
    Item,
    Items,
    Jump,
    KV,
    KVRow,
    Page,
    Section,
    Text,
    TextArea,
    TextField,
    Time,
  } from '../ui';

  // 브라우저: 상태, 저장된 로그인(결정 0006), 막는 곳(결정 0008), 사이트 위임(결정 0028). 짧은 앞의 셋은 두 단으로, 긴 사이트 위임은 그 아래 전체 너비로 둔다.
  // 브라우저 기능이 꺼져 있어도(snapshot.browser가 비어도) 사이트 위임은 보이고 쓸 수 있다.
  let { data }: { data: PageData } = $props();
  const s = $derived(data.snapshot);
  const b = $derived(s.browser);

  type Site = NonNullable<Snapshot['browser']>['sites'][number];
  type Grant = Snapshot['site_grants'][number];
  type Block = NonNullable<Snapshot['browser']>['recent_blocks'][number];

  // 생성된 타입은 이 칸들을 늘 있는 글로 적었지만 데몬에서는 비어 있을 수 있다.
  const username = (site: Site): string | null => site.username;
  const lastUsed = (g: Grant): string | null => g.last_used_at;
  const blockedWhy = (d: Block): string | null => d.blocked;

  // 위임 목록과 "n곳"은 지금 쓸 수 있는 것만 센다(서버가 정한 불리언).
  const live = $derived(s.site_grants.filter((g) => g.ui.active));
</script>

{#snippet grants()}
  <Section id="grants" title="사이트 위임" icon="link" hint={`${live.length}곳`}>
    {#snippet lead()}사람 없이 도는 일(후속 조치·반복 루틴·브리핑·작업)은 여기 있는 사이트만 브라우저로 엽니다. 한 번 위임한 사이트는 다시 묻지 않습니다.{/snippet}
    {#if live.length === 0}
      <Empty>위임한 사이트가 없습니다.</Empty>
    {:else}
      <Items>
        {#each live as g (g.id)}
          {@const used = lastUsed(g)}
          {#snippet grantHeading()}
            {g.host}
            <Text size="sm" tone="muted">(하위 도메인 포함)</Text>
          {/snippet}
          {#snippet grantEnd()}
            <Confirm
              action={`/grants/${g.id}/revoke`}
              label="거두기"
              question="이 사이트 위임을 거둡니다. 다음 실행부터 사람 없이 열지 않고 다시 묻습니다."
              confirmLabel="거두기"
              danger
            />
          {/snippet}
          <Item heading={grantHeading} icon="link" end={grantEnd}>
            <p>{g.ui.scope_text}</p>
            <p>
              {g.purpose}{' · '}{g.ui.origin_label}{' · '}<Time at={g.created_at} />
              {#if g.expires_at !== null}{' · '}<Time at={g.expires_at} />까지{/if}
              {' · '}쓴 횟수 {g.uses}
              {#if used !== null}{' · 마지막 '}<Time at={used} />{/if}
            </p>
          </Item>
        {/each}
      </Items>
    {/if}
    <h3>사이트 위임하기</h3>
    <ActionForm action="/grants/add">
      <TextField
        label="사이트 호스트"
        help="하위 도메인도 함께 덮습니다. 로그인이 다른 호스트(통합 인증)에서 이뤄지면 둘을 덮는 상위 호스트를 넣습니다."
        name="host"
        required
        width="md"
        placeholder="예: learning.example.ac.kr"
      />
      <TextField label="무엇에 쓰나(한 줄)" name="purpose" maxlength={120} width="md" placeholder="예: 과제 마감·제출 여부 확인" />
      <FieldSet legend="위임 범위">
        <CheckField name="login" label="세션이 끊기면 저장된 로그인으로 묻지 않고 다시 로그인" />
        <CheckField name="actions" label="폼 제출 같은 승인이 필요한 동작도 묻지 않기" />
      </FieldSet>
      <Button type="submit" variant="primary">위임하기</Button>
    </ActionForm>
    <Disclosure summary="위임은 어떻게 동작하나">
      <p>
        다른 사이트를 열려 하면 Discord 카드로 묻고, [이 사이트 계속 허용]을 누르면 여기에 더해집니다. 결제·탈퇴·비밀번호 바꾸기 같은
        동작은 위임해도 하지 않습니다.
      </p>
    </Disclosure>
  </Section>
{/snippet}

<Page title="브라우저" desc="Lucia가 쓰는 브라우저의 상태와 저장된 로그인, 사람 없이 갈 수 있는 사이트, 막는 곳을 관리합니다.">
  {#snippet actions()}
    {#if b !== null}<Button href="/browser/handover" icon="globe">화면 넘겨받기</Button>{/if}
  {/snippet}

  {#if b === null}
    <Callout tone="muted">
      <strong>브라우저가 꺼져 있습니다.</strong>
      브라우저 샌드박스(<code>deploy/create-browser-sandbox.sh</code>)를 만들고 설정 <code>[browser]</code>의 <code>enabled</code>를 켜면 씁니다.
    </Callout>
    <!-- 브라우저가 꺼져도 사이트 위임은 쓴다. 구역이 하나뿐이라 Columns 밖에 둔다(반쪽 너비로 남지 않게). -->
    {@render grants()}
  {:else}
    <Jump
      items={[
        { id: 'browser-status', label: '상태', count: 1 },
        { id: 'logins', label: '저장된 로그인', count: b.sites.length },
        { id: 'blocked', label: '막는 곳', count: b.blocked.length },
        { id: 'grants', label: '사이트 위임', count: live.length },
      ]}
    />
    <Columns>
      <Section id="browser-status" title="상태" icon="globe">
        <KV>
          <KVRow label="샌드박스">{b.sandbox}</KVRow>
          <KVRow label="지금">
            {#if b.running}
              <Badge tone="ok">열려 있음</Badge>
            {:else}
              <Badge>닫힘</Badge>{' '}<Text size="sm" tone="muted">쓸 때 열고 {b.idle_minutes}분 안 쓰면 닫습니다</Text>
            {/if}
          </KVRow>
        </KV>
        <Text size="sm" tone="muted" block>
          캡차·본인인증처럼 Lucia가 못 하는 단계는 <a href="/browser/handover">화면 넘겨받기</a>로 직접 합니다.
        </Text>
      </Section>

      <Section id="logins" title="저장된 로그인" icon="lock">
        {#if b.sites.length === 0}
          <Empty>로그인이 필요한 사이트가 아직 없습니다.</Empty>
        {:else}
          <Items>
            {#each b.sites as site (site.id)}
              {#snippet siteHeading()}
                {site.name}
                {#if site.name !== site.origin}<Text size="sm" tone="muted">{site.origin}</Text>{/if}
              {/snippet}
              {#snippet siteEnd()}
                {#if site.saved || username(site) !== null}
                  <Confirm
                    action={`/browser/forget?site=${encodeURIComponent(site.id)}`}
                    label="지우기"
                    question="저장된 비밀번호와 아이디를 지웁니다. 다음 로그인 때 다시 입력해야 합니다. 브라우저의 로그인 상태(쿠키)는 그대로입니다."
                    confirmLabel="지우기"
                    danger
                  />
                {/if}
              {/snippet}
              <Item heading={siteHeading} icon="shield" tone={site.saved ? 'ok' : 'muted'} end={siteEnd}>
                {#if site.saved}
                  저장된 로그인: {username(site) ?? '?'} · 다시 쓸 때마다 Discord 승인
                {:else}
                  저장된 로그인 없음 · 로그인이 필요하면 입력 링크를 보냅니다
                {/if}
              </Item>
            {/each}
          </Items>
        {/if}
      </Section>

      <Section id="blocked" title="막는 곳" icon="shield">
        {#snippet lead()}공개 인터넷은 열고, 아래만 막습니다(차단 목록 방식).{/snippet}
        <ActionForm action="/browser/blocked">
          <TextArea
            label="더 막을 도메인(한 줄에 하나, 하위 도메인 포함)"
            name="list"
            rows={5}
            placeholder="예: ads.example.com"
            value={b.blocked.join('\n')}
          />
          <Button type="submit">저장</Button>
        </ActionForm>
        {#if b.recent_blocks.length > 0}
          <h3>최근에 막은 연결</h3>
          <Items>
            {#each b.recent_blocks as d, i (i)}
              <Item title={`${d.host}:${d.port}`} icon="shield" tone="warn">
                <Time at={d.at} />{' · '}{blockedWhy(d) ?? ''}
              </Item>
            {/each}
          </Items>
        {/if}
        <Disclosure summary="늘 막는 곳">
          <p>
            사설·루프백·링크 로컬 같은 내부 주소, localhost·.local·.internal 같은 내부 이름과 점 없는 이름, Claude 계정 도메인, 80·443 밖의
            포트. 모든 연결은 데몬 프록시를 지나며, 이름을 한 번 풀어 그 주소로 연결합니다.
          </p>
        </Disclosure>
      </Section>
    </Columns>
    <!-- 사이트 위임은 목록과 폼이 길어 한 단에 넣으면 옆 단이 크게 빈다. 짧은 구역 셋을 두 단으로 맞추고, 이것은 그 아래 전체 너비로 둔다. -->
    {@render grants()}
  {/if}
</Page>
