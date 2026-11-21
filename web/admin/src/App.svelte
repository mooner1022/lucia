<script lang="ts">
  import { app, load } from './lib/app.svelte';
  import { isWide, pageFor } from './pages';
  import Standalone from './standalone/Standalone.svelte';
  import { Button, Callout, Shell, Solo, Text } from './ui';
</script>

{#if app.data?.kind === 'page'}
  {@const data = app.data}
  {@const View = pageFor(data)}
  <Shell {data} wide={isWide(data)}>
    {#key app.version}
      <!-- 화면 하나를 그리다 오류가 나도 틀(메뉴, 로그아웃)은 남긴다. 다른 화면으로 갈 수 있어야 한다. -->
      <svelte:boundary onerror={(error) => console.error(error)}>
        <View {data} />
        {#snippet failed(error)}
          <Callout tone="bad" title="이 화면을 그리지 못했습니다." role="alert">
            <p>{error instanceof Error ? error.message : String(error)}</p>
            <p>다른 화면은 메뉴에서 열 수 있습니다. 이 화면이 계속 이러면 데몬 로그와 함께 알려 주세요.</p>
          </Callout>
          <Button onclick={() => load()}>다시 불러오기</Button>
        {/snippet}
      </svelte:boundary>
    {/key}
  </Shell>
{:else if app.data?.kind === 'standalone'}
  {#key app.version}
    <Standalone data={app.data} />
  {/key}
{:else if app.error}
  <Solo title="화면을 불러오지 못했습니다" tone="bad">
    <Text tone="subtle" block>{app.error}</Text>
    <Button variant="primary" onclick={() => load()}>다시 시도</Button>
  </Solo>
{:else}
  <div class="sr-only" role="status">불러오는 중</div>
{/if}
