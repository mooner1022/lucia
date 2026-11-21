<script lang="ts">
  import type { PageData } from '../lib/types';
  import type { Snapshot } from '../lib/types.gen';
  import {
    ActionForm,
    Badge,
    Button,
    Callout,
    Columns,
    Confirm,
    Disclosure,
    Empty,
    Item,
    Items,
    Jump,
    Page,
    Picture,
    Section,
    SelectField,
    Text,
    TextField,
    Time,
  } from '../ui';

  // 이모지·스티커(결정 0045): 전용 앱에서 Lucia와 주고받는 그림의 목록. 올리기, 고치기, 지우기, 분류.
  // 이름·설명·태그는 사람이 넣은 글이다(화면은 글로만 넣는다).
  let { data }: { data: PageData } = $props();
  const catalog = $derived(data.snapshot.expressions);
  const rules = $derived(data.consts.expressions);

  type Catalog = NonNullable<Snapshot['expressions']>;
  type Expression = Catalog['emoji'][number];
  type Category = Catalog['categories'][number];
  type Kind = 'emoji' | 'sticker';

  // 종류의 이름과 한도는 서버가 준다(consts.expressions.kinds). 이모지·스티커 순이다.
  const kindOf = (key: Kind) => rules.kinds.find((k) => k.key === key);
  const kindLabel = (key: Kind) => kindOf(key)?.label ?? '';
  const kindMax = (key: Kind) => kindOf(key)?.max_count ?? 0;
  const kib = (key: Kind) => `${(kindOf(key)?.max_bytes ?? 0) / 1024} KiB`;

  // 생성된 타입은 분류의 아이콘을 늘 있는 글로 적었지만 비어 있을 수 있다.
  const iconOf = (cat: Category): string | null => cat.icon;

  const everything = $derived(catalog === null ? [] : [...catalog.emoji, ...catalog.stickers]);
  const categoryOptions = $derived([
    { value: '', label: '분류 없음' },
    ...(catalog?.categories ?? []).map((c) => ({ value: c.id, label: c.label })),
  ]);
  const iconOptions = $derived([
    { value: '', label: '없음' },
    ...everything.map((e) => ({
      value: e.id,
      label: e.kind === 'emoji' ? `:${e.name}:` : `${kindLabel('sticker')} ${e.name}`,
    })),
  ]);

  const categoryLabel = (id: string) => catalog?.categories.find((c) => c.id === id)?.label ?? '분류 없음';
  const alt = (kind: Kind, e: Expression) => `${kindLabel(kind)}: ${e.label === '' ? e.name : e.label}`;
</script>

{#snippet list(kind: Kind, id: string, items: Expression[], empty: string)}
  <Section {id} title={kindLabel(kind)} icon="smile" hint={`${items.length}개 · ${kindMax(kind)}개까지`}>
    {#if items.length === 0}
      <Empty>{empty}</Empty>
    {:else}
      <Items>
        {#each items as e (e.id)}
          {#snippet media()}
            <Picture src={`/expressions/file/${encodeURIComponent(e.file.id)}`} alt={alt(kind, e)} size="sm" />
          {/snippet}
          {#snippet heading()}
            {#if kind === 'emoji'}
              <code>:{e.name}:</code>{#if e.label !== ''}{' '}{e.label}{/if}
            {:else}
              {e.name}
            {/if}
            {#if e.file.animated}<Badge tone="info">움직임</Badge>{/if}
          {/snippet}
          {#snippet remove()}
            <Confirm
              action={`/expressions/${e.id}/delete`}
              label="지우기"
              question={`목록에서 지웁니다. 예전 메시지가 깨지지 않게 그림은 ${rules.keep_days}일 동안 남습니다.`}
              confirmLabel="지우기"
              danger
            />
          {/snippet}
          {#snippet edit()}
            <Disclosure kind="more" summary="고치기">
              <ActionForm action={`/expressions/${e.id}/update`}>
                {#if kind === 'emoji'}
                  <TextField label="보이는 이름" name="label" maxlength={32} autocomplete="off" value={e.label} />
                {/if}
                <TextField label="설명" name="description" maxlength={100} autocomplete="off" value={e.description} />
                <TextField label="태그(쉼표로 나눔)" name="tags" autocomplete="off" value={e.tags.join(', ')} />
                <SelectField label="분류" name="category" options={categoryOptions} value={e.category} />
                <Button type="submit">저장</Button>
              </ActionForm>
            </Disclosure>
          {/snippet}
          <Item {media} {heading} end={remove} extra={edit}>
            <p>
              {#if e.description === ''}설명 없음{:else}{e.description}{/if}
              {#if e.tags.length > 0}{' · 태그: '}{e.tags.join(', ')}{/if}
            </p>
            <p>
              {categoryLabel(e.category)}{' · '}{e.file.width}×{e.file.height}{' · '}{e.ui.size_text}{' · '}{e.ui.origin_label}{' · '}<Time
                at={e.created_at}
              />
            </p>
          </Item>
        {/each}
      </Items>
    {/if}
  </Section>
{/snippet}

<Page
  title="이모지·스티커"
  desc="전용 앱에서 Lucia와 주고받는 이모지와 스티커를 관리합니다. Lucia는 여기 적힌 이름과 설명을 보고 고릅니다."
>
  {#if catalog === null}
    <Callout tone="muted">
      <strong>이 데몬은 이모지·스티커를 다루지 않습니다.</strong>
      접속 API가 준비되면 여기에 목록이 나옵니다.
    </Callout>
  {:else}
    <Jump
      items={[
        { id: 'emoji', label: '이모지', count: catalog.emoji.length },
        { id: 'stickers', label: '스티커', count: catalog.stickers.length },
        { id: 'categories', label: '분류', count: catalog.categories.length },
        { id: 'expr-add', label: '새로 올리기' },
      ]}
    />
    <!-- 목록(이모지, 스티커)을 먼저 보이고, 분류와 올리기 폼은 그 뒤에 둔다. 올리기 폼이 가장 길어 맨 끝에 두면 두 단이 맞는다.
         목록이 아주 길어져도(이모지 200개, 스티커 100개까지) 위의 건너뛰기 링크로 올리기와 분류에 닿는다. -->
    <Columns>
      {@render list('emoji', 'emoji', catalog.emoji, '이모지가 아직 없습니다.')}
      {@render list('sticker', 'stickers', catalog.stickers, '스티커가 아직 없습니다.')}

      <Section id="categories" title="분류" icon="list" hint={`${catalog.categories.length}개 · ${rules.max_categories}개까지`}>
        {#snippet lead()}앱의 고르기 화면에서 묶음(탭)으로 보입니다. 순서 숫자가 작은 것이 앞에 옵니다.{/snippet}
        {#if catalog.categories.length === 0}
          <Empty>분류가 아직 없습니다.</Empty>
        {:else}
          <Items>
            {#each catalog.categories as cat (cat.id)}
              {@const count = everything.filter((e) => e.category === cat.id).length}
              {@const iconName = everything.find((e) => e.id === iconOf(cat))?.name}
              {#snippet heading()}
                {cat.label}
                <Text size="sm" tone="muted">{cat.id}</Text>
              {/snippet}
              {#snippet remove()}
                <Confirm
                  action={`/expressions/categories/${cat.id}/delete`}
                  label="지우기"
                  question="분류를 지웁니다. 이 분류의 이모지·스티커는 지워지지 않고 분류 없음이 됩니다."
                  confirmLabel="지우기"
                  danger
                />
              {/snippet}
              {#snippet edit()}
                <Disclosure kind="more" summary="고치기">
                  <ActionForm action="/expressions/categories/save">
                    <input type="hidden" name="id" value={cat.id} />
                    <TextField label="이름" name="label" required maxlength={32} autocomplete="off" value={cat.label} />
                    <TextField label="순서" type="number" name="sort" min={0} max={999} value={cat.sort} width="sm" />
                    <SelectField label="탭 아이콘" name="icon" options={iconOptions} value={iconOf(cat) ?? ''} />
                    <Button type="submit">저장</Button>
                  </ActionForm>
                </Disclosure>
              {/snippet}
              <Item {heading} icon="list" end={remove} extra={edit}>
                순서 {cat.sort}{' · '}항목 {count}개{#if iconName !== undefined}{' · 탭 아이콘 '}{iconName}{/if}
              </Item>
            {/each}
          </Items>
        {/if}
        <h3>분류 만들기</h3>
        <ActionForm action="/expressions/categories/save">
          <TextField
            label="분류 ID"
            help="영문 소문자·숫자·_ 로 24자까지. 만든 뒤에는 바꿀 수 없습니다."
            name="id"
            required
            maxlength={24}
            autocomplete="off"
            placeholder="예: lucia"
          />
          <TextField label="이름" name="label" required maxlength={32} autocomplete="off" placeholder="예: 루시아" />
          <Button type="submit" variant="primary">만들기</Button>
        </ActionForm>
      </Section>

      <Section id="expr-add" title="새로 올리기" icon="upload">
        {#snippet lead()}그림은 고치지 않고 검사만 합니다. 이모지는 PNG·WebP·GIF, 정사각형, 한 변 32~256px(128 권장), {kib('emoji')}까지. 스티커는 PNG·APNG·WebP·GIF, 정사각형, 한 변 160~640px(320 권장), {kib('sticker')}까지.{/snippet}
        <ActionForm action="/expressions/add" multipart>
          <SelectField
            label="종류"
            name="kind"
            options={[
              { value: 'emoji', label: '이모지(글 안에 섞는 작은 그림)' },
              { value: 'sticker', label: '스티커(메시지 하나를 차지하는 큰 그림)' },
            ]}
          />
          <TextField label="그림 파일" type="file" name="file" accept="image/png,image/apng,image/webp,image/gif" required />
          <TextField
            label="이름"
            help="이모지는 영문 소문자·숫자·_ 로 2~32자(글에 :이름: 으로 씁니다). 스티커는 2~30자(한국어 가능). 올린 뒤에는 바꿀 수 없습니다."
            name="name"
            required
            maxlength={32}
            autocomplete="off"
            placeholder="예: lucia_smile"
          />
          <TextField
            label="보이는 이름(이모지만)"
            help="앱의 고르기 화면에 보입니다. 한국어로 적어도 됩니다."
            name="label"
            maxlength={32}
            autocomplete="off"
            placeholder="예: 루시아 웃음"
          />
          <TextField
            label="설명"
            help="Lucia가 이 설명을 보고 언제 쓸지 고릅니다. 100자까지."
            name="description"
            maxlength={100}
            autocomplete="off"
            placeholder="예: 기분 좋게 웃는 얼굴"
          />
          <TextField
            label="태그"
            help="쉼표로 나눕니다. 10개까지, 각 20자까지. 앱에서 찾을 때 씁니다."
            name="tags"
            autocomplete="off"
            placeholder="예: 웃음, 좋아, 기쁨"
          />
          <SelectField label="분류" name="category" options={categoryOptions} />
          <Button type="submit" variant="primary" icon="upload">올리기</Button>
        </ActionForm>
        <Disclosure summary="Lucia에게 맡기기">
          <p>
            대화에서 그림을 붙이고 "이걸 이모지로 추가해 줘"라고 하면 Lucia가 같은 목록에 등록합니다. 사진처럼 규격에 맞지 않는 그림은 Lucia가
            스크립트로 줄이고 PNG로 바꾼 뒤 등록합니다.
          </p>
        </Disclosure>
      </Section>
    </Columns>
  {/if}
</Page>
