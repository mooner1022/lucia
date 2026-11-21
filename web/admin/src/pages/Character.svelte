<script lang="ts">
  import type { PageData } from '../lib/types';
  import type { ExtraCharacterEdit } from '../lib/types.gen';
  import {
    ActionForm,
    Button,
    Callout,
    Cluster,
    Columns,
    Confirm,
    Disclosure,
    Empty,
    Item,
    Items,
    Page,
    Section,
    SelectField,
    Text,
    TextArea,
    TextField,
    Time,
  } from '../ui';

  // 캐릭터: 파일 편집(이 글 전체가 매 실행 모델에 들어간다)과 그 변경 기록, 주 모델, 메시지 나눠 보내기.
  // 파일을 못 읽었으면 편집 칸 대신 까닭을 보이고, 나머지는 그대로 쓴다.
  let { data }: { data: PageData<ExtraCharacterEdit> } = $props();
  const x = $derived(data.extra);
  const model = $derived(x.model);

  const modelOptions = $derived([
    { value: 'default', label: `기본(설정 파일 값: ${model.config_default})` },
    ...model.choices.map((c) => ({ value: c.key, label: c.label })),
  ]);
  const paceOptions = $derived(x.paces.map((p) => ({ value: p.key, label: p.label })));
</script>

{#snippet compactFoot()}
  <Cluster justify="between">
    <Confirm
      action="/run/compact"
      label="지금 정리해 적용"
      question="상주 세션의 대화를 요약해 줄이고(모델 1회), 다음 대화부터 지금 캐릭터 파일로 시작합니다. 실패하면 새 세션으로 넘어가고, 기억과 최근 대화로 이어 갑니다."
      confirmLabel="정리하기"
    />
    <Text size="sm" tone="muted">세션 정리 상태는 시스템 화면에서 봅니다.</Text>
  </Cluster>
{/snippet}

<Page title="캐릭터" desc="Lucia의 이름·말투·관계를 정합니다. 이 글 전체가 매 대화와 브리핑의 시스템 프롬프트 뒤에 붙습니다.">
  {#if x.error !== null}
    <Callout tone="warn">캐릭터 파일을 읽지 못했습니다: {x.error}</Callout>
  {:else}
    <Section
      id="character-file"
      title="캐릭터 파일"
      icon="chat"
      hint={`${x.content_bytes ?? 0} / ${x.max_bytes}바이트`}
      foot={compactFoot}
    >
      <Callout tone="info">
        말투, 이름, 관계 같은 캐릭터만 씁니다. 판단, 정확성, 승인 규칙은 캐릭터로 바뀌지 않습니다. 고친 내용은 다음 세션 정리나 새 세션부터
        적용됩니다.
      </Callout>
      <ActionForm action="/character">
        <input type="hidden" name="base" value={x.fingerprint ?? ''} />
        <TextArea label="내용(마크다운)" name="content" rows={26} spellcheck={false} required mono value={x.content ?? ''} />
        <TextField label="바꾼 까닭(기록용, 비워도 됩니다)" name="note" maxlength={200} placeholder="예: 이름과 말투를 정했다">
          {#snippet action()}<Button type="submit">저장</Button>{/snippet}
        </TextField>
      </ActionForm>
      {#if x.path !== null}
        <Text size="sm" tone="muted" block>파일: <code>{x.path}</code></Text>
      {/if}
      <Disclosure summary="캐릭터를 쓸 때 알아 둘 것">
        <ul>
          <li>
            말투, 이름, 관계 같은 캐릭터만 씁니다. 사용자에 대한 사실·취향·일정처럼 바뀌는 내용은 넣지 말고 Lucia의 기억에 맡깁니다.
          </li>
          <li>판단, 정확성, 승인 규칙은 캐릭터로 바뀌지 않습니다. ‘캐릭터보다 우선하는 것’ 절은 남겨 두세요.</li>
          <li>전체가 매 실행 모델에 들어가므로 짧을수록 좋습니다. 메모나 주석도 그대로 들어갑니다.</li>
          <li>
            상주 대화는 세션을 시작할 때의 캐릭터를 기록해 두고 이어 씁니다. 그래서 고친 내용은 다음 세션 정리(압축)나 새 세션부터 적용됩니다.
            바로 적용하려면 저장한 뒤 ‘지금 정리해 적용’을 누릅니다.
          </li>
          <li>Lucia는 이 파일을 고칠 수 없습니다(읽기 전용). 고치는 곳은 이 화면뿐입니다.</li>
        </ul>
      </Disclosure>
    </Section>
  {/if}

  <Columns>
    <Section id="character-history" title="변경 기록" icon="clock">
      {#if x.history.length === 0}
        <Empty>아직 이 화면에서 고친 적이 없습니다.</Empty>
      {:else}
        <Items>
          {#each x.history as r (r.id)}
            <Item href={`/character/rev/${r.id}`} title={`개정 ${r.id} · ${r.note}`}>
              <Time at={r.at} />{' · '}{r.ui.bytes}바이트
            </Item>
          {/each}
        </Items>
      {/if}
    </Section>

    <Section id="main-model" title="주 모델" icon="memory" hint={model.label}>
      {#snippet lead()}대화, 맡긴 작업, 브리핑과 정기 보고처럼 Lucia가 생각하는 실행에 쓰는 모델입니다. 회고와 메일 분류에 쓰는 작은 모델은 설정 파일에서 정합니다.{/snippet}
      <ActionForm action="/character/model">
        <SelectField label="모델" help={model.change_note} name="model" options={modelOptions} value={model.picked ?? 'default'}>
          {#snippet action()}<Button type="submit">저장</Button>{/snippet}
        </SelectField>
      </ActionForm>
      <Disclosure summary="모델을 고를 때 알아 둘 것">
        <ul>
          {#each model.choices as c (c.key)}
            <li><strong>{c.label}</strong>{': '}{c.about}</li>
          {/each}
          <li>이름은 그 계열의 최신 모델을 가리킵니다. 실제로 쓴 모델은 시스템 화면의 사용량에서 봅니다.</li>
          <li>
            유능한 모델일수록 구독 사용량과 자동 실행 한도(설정의 하루·주간 한도)를 빨리 씁니다. 사람 없이 도는 브리핑과 정기 보고도 고른 모델로
            돕니다.
          </li>
          <li>
            모델을 바꿀 때마다(되돌릴 때도) 새 모델이 대화를 다시 읽습니다. 대화 맥락이 클 때는 시스템 화면에서 ‘지금 정리’를 먼저 하면 다시 읽는
            양이 줄어듭니다.
          </li>
          <li>Discord와 앱에서는 <code>/모델</code> 명령으로도 보고 바꿀 수 있습니다.</li>
        </ul>
      </Disclosure>
    </Section>

    <Section id="pace" title="메시지 나눠 보내기" icon="chat">
      {#snippet lead()}Lucia는 일상 대화를 메신저처럼 짧은 메시지 몇 개로 나눠 보냅니다. 메시지 사이를 사람이 치는 시간만큼 띄우고, 그동안 입력 중 표시를 켭니다.{/snippet}
      <ActionForm action="/character/pace">
        <SelectField
          label="메시지 사이 간격"
          help="다음 메시지가 길수록 조금 더 띄우고(한 번에 몇 초까지), 모델이 이어 쓰는 동안 흐른 시간은 빼고 모자란 만큼만 기다립니다."
          name="pace"
          options={paceOptions}
          value={x.pace}
        >
          {#snippet action()}<Button type="submit">저장</Button>{/snippet}
        </SelectField>
      </ActionForm>
    </Section>
  </Columns>
</Page>
