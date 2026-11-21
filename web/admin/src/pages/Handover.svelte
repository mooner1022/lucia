<script lang="ts">
  import { load } from '../lib/app.svelte';
  import type { PageData } from '../lib/types';
  import type { ExtraBrowserHandover } from '../lib/types.gen';
  import {
    ActionForm,
    Badge,
    Button,
    Callout,
    Cluster,
    FieldSet,
    Item,
    Items,
    Page,
    PostButton,
    Screen,
    Section,
    Text,
    TextField,
  } from '../ui';

  // 화면 넘겨받기(결정 0017): 캡차·본인인증처럼 Lucia가 못 하는 단계를 사람이 직접 한다.
  // 화면은 이미지로 보이고, 누르면 그 자리의 좌표가 간다(Screen). 나머지 동작(글 넣기, 키, 스크롤)은 작은 폼이다.
  let { data }: { data: PageData<ExtraBrowserHandover> } = $props();
  const x = $derived(data.extra);
  const keys = $derived(data.consts.browser.handover_keys);
  const maxMinutes = $derived(Math.round(data.consts.browser.handover_max_secs / 60));

  const tabQuery = $derived(encodeURIComponent(x.tab ?? ''));
  const current = $derived(x.tabs.find((t) => t.id === x.tab));
  // 그림 주소 뒤의 값은 데이터를 받을 때마다 바뀌어야 브라우저가 새 그림을 받는다.
  const frame = (tab: string) =>
    `/browser/handover/frame/${encodeURIComponent(tab)}.jpg?v=${encodeURIComponent(data.snapshot.now)}`;

  // 예전에는 머리의 meta refresh였다. 이제는 화면이 몇 초마다 조용히 다시 물어 새 그림을 받는다.
  $effect(() => {
    const seconds = x.refresh;
    if (seconds === null || !(seconds > 0)) return;
    const timer = setInterval(() => void load({ quiet: true, keepFlash: true }), seconds * 1000);
    return () => clearInterval(timer);
  });
</script>

<Page
  title="브라우저 화면 넘겨받기"
  desc="캡차·본인인증·복잡한 로그인처럼 Lucia가 할 수 없는 단계를 직접 합니다."
  back={{ href: '/browser', label: '브라우저' }}
>
  {#if !x.available}
    <Callout tone="muted">브라우저가 꺼져 있습니다.</Callout>
  {:else if x.left_secs !== null}
    {@const left = x.left_secs}
    <Section id="handover" title="넘겨받는 중" icon="globe" tone="warn">
      {#snippet end()}
        <Badge tone="warn">남은 시간 {Math.floor(left / 60)}분 {left % 60}초</Badge>
      {/snippet}
      <Text size="sm" tone="muted" block>
        그동안 Lucia는 브라우저를 쓰지 않습니다. 넣은 글과 누른 자리는 기록하지 않습니다(시작·끝과 주소만 남깁니다).
      </Text>
      <Cluster gap={2}>
        <PostButton action="/browser/handover/end" variant="primary" size="md">끝내기</PostButton>
        {#if x.auto}
          <Button href={`/browser/handover?tab=${tabQuery}`}>자동 새로고침 끄기</Button>
        {:else}
          <Button href={`/browser/handover?tab=${tabQuery}&auto=1`}>3초마다 새로고침</Button>
          <Button href={`/browser/handover?tab=${tabQuery}`} icon="refresh">화면 새로고침</Button>
        {/if}
      </Cluster>
    </Section>

    {#if x.tabs.length > 1}
      <Section id="tabs" title="브라우저 탭" icon="list">
        <Items>
          {#each x.tabs as t (t.id)}
            {@const here = t.id === x.tab}
            {#snippet tabHeading()}
              {@const name = t.title === '' ? '(제목 없음)' : t.title}
              {#if here}
                <strong>{name}</strong>
                <Badge tone="info">보는 중</Badge>
              {:else}
                <a href={`/browser/handover?tab=${encodeURIComponent(t.id)}`}>{name}</a>
              {/if}
            {/snippet}
            <Item heading={tabHeading}>{t.url}</Item>
          {/each}
        </Items>
      </Section>
    {/if}

    {#if x.tab !== null}
      {@const tab = x.tab}
      <Section id="handover-screen" title="화면" icon="search" hint={`주소: ${current?.url ?? ''}`}>
        <Screen
          action="/browser/handover/act"
          src={frame(tab)}
          alt="브라우저 화면(누르면 그 자리를 누릅니다)"
          fields={{ tab, kind: 'click' }}
        />
        <ActionForm action="/browser/handover/act" track={false}>
          <input type="hidden" name="tab" value={tab} />
          <input type="hidden" name="kind" value="text" />
          <TextField label="글 넣기(초점이 있는 칸에)" name="text" maxlength={2000} autocomplete="off">
            {#snippet action()}<Button type="submit">넣기</Button>{/snippet}
          </TextField>
        </ActionForm>
        <FieldSet legend="키와 스크롤">
          <Cluster gap={2}>
            {#each keys as k (k.key)}
              <PostButton action="/browser/handover/act" size="md" fields={{ tab, kind: 'key', key: k.key }}>{k.label}</PostButton>
            {/each}
            <!-- 스크롤 값은 예전 화면에 적힌 숫자 그대로다(처리기는 ±3000으로 자른다). -->
            <PostButton action="/browser/handover/act" size="md" fields={{ tab, kind: 'scroll', dy: '-600' }}>위로</PostButton>
            <PostButton action="/browser/handover/act" size="md" fields={{ tab, kind: 'scroll', dy: '600' }}>아래로</PostButton>
          </Cluster>
        </FieldSet>
      </Section>
    {/if}
  {:else}
    <Section id="handover" title="넘겨받기" icon="globe">
      <p>
        넘겨받는 동안(최대 {maxMinutes}분) Lucia는 브라우저를 쓰지 않고, 새 창(본인인증 팝업 등)도 닫지 않고 탭으로 남깁니다. 끝내면 그 탭들은
        닫고 로그인된 상태는 남습니다.
      </p>
      <PostButton action="/browser/handover/start" variant="primary" size="md">넘겨받기</PostButton>
    </Section>
  {/if}
</Page>
