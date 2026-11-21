# 관리 화면 짜는 법

관리 화면(결정 0061)은 정적 Svelte 5 빌드다. 이 문서는 화면(`src/pages`, `src/standalone`)을 쓰거나 고치는 쪽이 지킬 약속이다.
기준 화면은 `src/pages/Connections.svelte`의 Discord 두 구역이다. 모르겠으면 그것을 따라 한다.
부품의 모습(색, 꼴, 문양, 글꼴)은 `design.md`의 디자인 언어를 따른다(결정 0062). 부품을 고치거나 새로 만들 때는 그 문서를 먼저 읽는다.

## 1. 큰 그림

- **주소는 서버가 정한다.** 화면은 지금 주소를 JSON으로 다시 물어 받은 것을 그린다(`src/lib/app.svelte.ts`). 화면 코드는 주소를 해석하지 않는다.
- **화면 하나 = 파일 하나.** `src/pages/index.ts`가 서버의 답(`data.page`, `data.extra.view`)으로 파일을 고른다. 화면은 `let { data }: { data: PageData } = $props();` 하나만 받는다.
- **쓰기 동작은 예전 주소 그대로 폼으로 보낸다.** 서버의 처리기는 바뀌지 않았다. `csrf`와 돌아올 곳(`back`)은 `ActionForm`이 보낼 때 붙인다. 화면에 적지 않는다.
- **모양은 부품이 정한다.** 화면에는 `class`, `style`, `<style>`이 없다. `tests/lint.test.ts`가 막는다. 부품이 모자라면 화면에서 때우지 말고 알린다(9장).

## 2. 데이터

| 어디 | 무엇 |
|---|---|
| `data.snapshot` | 모든 화면이 함께 보는 상태(타입 `Snapshot`, `src/lib/types.gen.ts`). 예전 화면 코드의 `s`. |
| `data.extra` | 화면마다 다른 덧붙임(기억 항목, 스킬, 캐릭터 파일, 넘겨받기 상태). 타입은 `types.gen.ts`의 `Extra…`. `PageData<ExtraMemoryEntry>`처럼 쓴다. |
| `data.consts` | 데몬의 상수와 이름표(한도, 범위 이름, 권한 모드 설명 …). |
| 항목의 `ui` | 데몬이 계산해 붙인 값(이름, 결, 설명). **`docs/derived.md`에 전부 있다.** |
| `data.notices`, `data.overall`, `data.pending` | 홈의 "확인할 것", 전체 상태, 승인 대기 수. |

- **데몬이 주는 값을 다시 만들지 않는다.** 예전 화면 코드가 데몬의 함수(`crate::…`)를 불러 얻던 이름·결·설명은 `ui`나 `consts`에 있다. 화면에서 표를 새로 만들지 않는다. 없으면 알린다(9장).
- **열거형의 꼴**: 값 없는 것은 글(`"NotConfigured"`), 값 있는 것은 `{ "Stopped": { "reason": "…" } }`. `Result`는 `{ "Ok": … }` 또는 `{ "Err": "…" }`. 짝은 배열이다(`watches[i]`는 `[감시, 원천 상태 | null]`, `clients[i]`는 `[클라이언트, 연결됨]`).
- **타입은 시험 데이터에서 만든 것이라** 열거형 값이 `string`이고, 시험 데이터에서 늘 비어 있던 칸은 `unknown`이다. 그런 칸은 예전 화면 코드와 데몬의 타입을 보고 좁혀 쓴다.
- **시각**은 `<Time at={…} />`로만 적는다(데몬의 시간대, 상대 표기 + 정확한 시각). 숫자·돈·크기는 `src/lib/format.ts`의 `num`, `usd`, `bytes`.

## 3. 화면의 뼈대

```svelte
<Page title="연결" desc="한 줄 설명">
  {#snippet actions()}…오른쪽 단추…{/snippet}
  {#snippet tabs()}<Tabs label="설정" items={…} />{/snippet}
  <Columns>                        <!-- 넓으면 두 단(높이를 저절로 맞춘다), 좁으면 한 단 -->
    <Section …/> <Section …/> <Section …/>
  </Columns>
</Page>
```

- `Page`: 제목(h1)은 하나. 설명에 마크업이 필요하면 `lead` 스니펫. 하위 화면(항목 하나, 개정 하나)은 `back={{ href, label }}`로 돌아갈 화면을 준다.
- `Section id title icon? hint? tone?`: 구역 하나(h2). `id`는 화면 안에서 유일. 스니펫 `end`(제목 줄 오른쪽 배지·단추), `lead`(제목 아래 한 줄), `foot`(아래 띠: 그 구역을 지우거나 끊는 단추처럼 본문과 가를 것). 본문은 세로로 쌓인다.
  - 스니펫 안이 조건에 따라 비면(`{#snippet foot()}{#if …}…{/if}{/snippet}`) 띠나 칸은 저절로 사라진다. `Item`의 `end`·`extra`·본문도 같다.
- 구역 안의 작은 제목은 `<h3>`. 그 아래는 `<h4>`.
- 구역이 여럿이면 `Columns` 안에 차례로 둔다. 두 단으로 나누는 것과 높이 맞추기는 부품이 한다(구역을 손으로 왼쪽·오른쪽에 나눠 담지 않는다). 중요한 것부터 적는다: 위에서 아래로, 그다음 옆 단으로 이어진다. 화면 전체 너비를 써야 하는 것(상태 타일 줄, 넓은 표, 긴 목록)은 `Columns` 밖에 둔다.
- 구역 안의 배치는 세 가지뿐이다.
  - `Stack gap={1..6}`: 세로로 쌓기.
  - `Cluster gap justify="start|between|end" align`: 가로로 늘어놓기. 모자라면 다음 줄로 넘어간다.
  - `Grid min="xs|sm|md|lg" gap`: 칸의 가장 좁은 너비만 정하는 격자. 칸 수를 정하지 않는다.
- 설정 화면의 보조 탭은 `SETTINGS_TABS`(`src/lib/nav.ts`)로 `Tabs`를 만든다. 한 화면 안의 구역 건너뛰기는 `Jump`.

## 4. 보여 주는 부품

| 부품 | 쓰임 |
|---|---|
| `Badge tone` | 상태 배지(아이콘 + 글). `plain`이면 아이콘 없이. |
| `Dot tone` | 장식 점. 뜻은 곁의 글이 말한다. |
| `Text size tone weight mono block` | 흐린 글·작은 글·고정폭. `block`이면 문단(`<p>`). |
| `KV` + `KVRow label` | 이름–값 목록. |
| `Items` + `Item` | 줄 목록. `Item title` 또는 `heading` 스니펫, `icon`·`tone`(앞자리의 아이콘) 또는 `media` 스니펫(앞자리의 작은 그림 `<Picture size="sm">`. 그림을 `heading` 안에 넣지 않는다), 본문(제목 아래 작은 글), `end`(오른쪽 배지·단추·시각), `extra`(줄 아래, 제목의 왼쪽 선에 맞춰). `href`를 주면 줄 전체가 링크다(끝에 화살표. 그 줄에는 단추를 두지 않는다). |
| `Table head` + `Row` + `Cell` | 표. 좁으면 줄마다 "이름: 값" 카드로 바뀐다. 대표 칸에 `main`. |
| `Stat label value icon? tone? href?` | 상태 타일(홈). `<Grid min="xs" stretch>` 안에 둔다(높이가 맞는다). |
| `Steps` + `Step title state` | 차례대로 하는 일(연결 절차). `state`: `done`·`current`·`todo`. 번호는 저절로 붙는다. 단계의 폼·단추는 Step 안에 둔다. 초기 설정 화면(`Start.svelte`)은 단계마다 길어서 `Step` 대신 구역(`Section`) 하나씩과 `Jump`로 짰다. |
| `Screen action src alt fields` | 원격 화면(화면 넘겨받기). 그림을 누르면 그 자리의 좌표를 보낸다. |
| `Progress value max label` | 쓴 양과 한도. |
| `Callout tone title?` | 눈에 띄어야 하는 안내. |
| `Empty` | 목록이 비었을 때 한 줄. |
| `Disclosure summary kind` | 접어 두기. `about`: 긴 설명("자세히"). `more`: 고급 설정이나 덧붙는 폼. |
| `CodeBlock text tall?` | 긴 글·코드 상자. |
| `Time at` | 시각. |
| `LastRun run` | "지금 실행" 동작의 마지막 결과 한 줄. |
| `Picture src alt size` | 그림. |
| `Icon name` | 아이콘(`src/ui/icons.ts`의 이름). 결(`Tone`)에 맞는 아이콘 이름은 `TONE_ICON[tone]`(`'../ui'`에서 가져온다). |

**글이 흐르는 곳과 쌓이는 곳.** `Callout`, `Item`의 본문, `KVRow`, `Cell`, `Section`의 `lead`는 글이 문장으로 흐른다(굵은 글·코드·배지를 섞어도 된다. 줄을 나누려면 `<p>`). `Section`의 본문, `Disclosure`, `Stack`은 안의 것을 세로로 쌓는다: 거기에 글을 둘 때는 꼭 `<p>`나 `Text block`으로 감싼다(감싸지 않은 글에 굵은 글이 섞이면 조각마다 줄이 끊긴다).

날것으로 써도 되는 태그: `p`, `span`, `strong`, `em`, `code`, `a`, `ul`/`ol`/`li`, `h3`, `h4`, `br`, `input type="hidden"`. 나머지는 부품을 쓴다.

## 5. 폼

- **`ActionForm action`**: 서버로 보내는 폼. 안에 입력 부품과 `<Button type="submit">`을 둔다.
  - `method="get"`: 찾기처럼 주소의 질의로 가는 폼(`track={false}`와 함께).
  - `multipart`: 파일을 올리는 폼.
  - `inline`: 줄 안에 놓이는 작은 폼.
  - `stay`: 서버가 화면 안의 다른 곳으로 가라고 해도(예전 처리기는 늘 `/connections`로 돌려보낸다) **지금 화면에 남고** 안내만 가져온다. 같은 처리기를 두 화면에서 부를 때 쓴다(초기 설정 화면, 결정 0064). 화면이 정하는 것이라 서버가 주는 값으로 열린 리다이렉트가 생기지 않는다. 바깥 주소(Google 동의 화면)와 로그인으로 가라는 답은 `stay`여도 그대로 따른다. 서버가 "하지 못했다"고 답하면 화면을 다시 그리지 않아 친 값이 남는다.
- **`PostButton action fields? variant size icon stay?`**: 단추 하나짜리 폼(지금 실행, 켜기, 멈추기). 숨은 값은 `fields={{ kind: 'x' }}`.
- **`Confirm action label question confirmLabel? danger fields? stay?`**: 되돌리기 어려운 동작. 첫 단추는 묻는 창을 열고, 창 안의 단추가 보낸다. 창 안에 입력이 더 필요하면 children에 둔다.
  - `PostButton`과 `Confirm`의 `stay`는 `ActionForm`의 것을 그대로 이어 준다.
- 입력: `TextField`(`type="password|number|file|url|search"`도), `TextArea`, `SelectField options value`, `CheckField`(`type="radio"`도), 여럿을 묶을 때 `FieldSet legend`.
  - `label`은 꼭 준다. 눈에 안 보여야 하면 `hideLabel`.
  - `name`, `required`, `min`, `max`, `maxlength`, `inputmode`, `autocomplete`, `accept`, `rows`, `placeholder`는 **예전 화면과 똑같이** 준다. 기능 보존 시험이 이 속성까지 본다(8장).
  - 입력 옆의 단추는 `action` 스니펫(`TextField`, `SelectField`). 좁으면 아래로 내려간다.
  - 좁은 값(숫자 몇 자리)은 `width="xs|sm|md"`.
- `ActionForm`에 바로 놓인 `Button`은 제 너비만 쓴다(가득 채우려면 `block`). 단추가 둘 이상이면 `Cluster`로 묶는다.
- **폼 안의 폼은 안 된다.** `Confirm`과 `PostButton`은 그 자체가 폼이므로 `ActionForm` 안에 넣지 않는다.
- 줄(`Item`)에 폼이 딸린 펼침이 있으면 `end`가 아니라 `extra`에 둔다(줄 아래에 제목의 왼쪽 선부터 끝까지 쓴다).
- 두 단 배치(`Columns`)에는 높이가 비슷한 구역끼리만 넣는다. 한 구역이 나머지보다 훨씬 길면 밖으로 꺼내 전체 너비로 둔다. 순서는 상태·목록 먼저, 폼은 뒤(`design.md` 8장).
- 숫자를 견주는 표는 `Table`의 `num`과 `Cell num`으로 오른쪽에 맞춘다.
- 서버가 답한 안내(성공·실패)는 틀이 위에 보인다. 화면이 따로 다루지 않는다.

## 6. 내용

- **기능을 없애지 않는다.** 있던 폼, 입력, 링크, 보이던 데이터는 모두 있어야 하고, 언제 보이는지(조건)도 같아야 한다. 기준은 개편 전 화면에서 뽑아 둔 목록이다(`tests/parity_baseline.txt`, `fixtures/baseline-by-page.json`). 기능을 더할 때는 데몬의 시험 데이터(`crates/luciad/src/admin/fixtures/samples.rs`)에 그 데이터를 넣고 시험 데이터 파일을 다시 만든다.
- **있던 글을 줄이거나 바꾸지 않는다.** 라벨, 도움말, 안내, 묻는 말은 개편 전 화면의 것을 그대로 옮겼고 시험이 지킨다(`tests/texts.json`). 새로 쓰는 글도 같은 말투(짧은 평서문, 존댓말)로 쓴다.
- **배치는 다시 짠다.** 예전 화면의 `div.cols`, `field-row`, `spacer` 같은 틀을 흉내 내지 않는다. 이 문서의 부품으로 읽기 좋게 놓는다.
  - 상태와 할 일을 먼저, 설정 폼은 그 뒤, 긴 설명은 `Disclosure`.
  - 한 구역에 폼이 여럿이면 각각을 눈으로 가를 수 있게 `h3`나 `Disclosure kind="more"`로 나눈다. 예전 화면에 소제목이 없었으면 그 폼의 라벨에 있는 말로 짧은 명사구 소제목을 붙여도 된다(문장을 새로 짓지는 않는다).
  - 목록이 길어질 수 있으면 `Items`나 `Table`. 가로로 여러 값을 늘어놓지 않는다.
- 바깥 글(메일 제목, 웹 결과, 다른 사람의 글)은 글로만 넣는다. `{@html}`은 쓰지 않는다.
- 주석은 한국어, 식별자는 영어. 주석에는 까닭만 적는다.

## 7. Svelte 5에서 걸리는 곳

- **블록 가장자리의 공백은 사라진다.** `{#if}`·`{#each}`나 태그의 맨 앞·맨 뒤 공백이 없어져 낱말이 붙는다. 이음매는 표현식으로 적는다: `{' · '}`, `{', '}`, `{' '}`.
- 속성 값에 `{`가 들어가면 표현식으로 준다: `placeholder={'{"installed": …} 전체'}`.
- `{#each … (key)}`의 열쇠는 그 목록에서 겹치지 않는 값만 쓴다(`id`, `slug`). 겹칠 수 있으면 열쇠를 빼거나 순번을 쓴다.
- 데이터가 바뀌면 화면이 다시 그려진다. 값을 계산할 때는 `$derived`를 쓰고, `data`에서 꺼낸 값을 `let`/`const`에 한 번 담아 두지 않는다.
- 타입: `types.gen.ts`는 시험 데이터에서 만든 어림이다. 비어 있을 수 있는 칸(`Option`)은 `| null`로 적혀 있거나, 적혀 있지 않아도 비어 올 수 있다. 예전 화면 코드가 `if let Some(…)`으로 다루던 칸은 화면에서도 null을 다룬다.

## 8. 확인

`export PATH=~/.local/opt/node/bin:$PATH` 뒤에 `web/admin`에서:

```sh
npx vitest run tests/parity.test.ts -t "있다 > /connections$"   # 그 화면의 기능 보존. 이름은 fixtures/baseline-by-page.json의 열쇠("/" , "/work", "memory/entry" …)
npx vitest run tests/text.test.ts -t "connections.rs"          # 예전 화면의 글이 그대로 있는지(이름은 예전 화면 코드 파일)
npx vitest run tests/lint.test.ts tests/escape.test.ts          # 화면 규칙, 바깥 글이 마크업으로 들어가지 않는지
npx svelte-check --tsconfig ./tsconfig.json --fail-on-warnings  # 타입
```

- 기능 보존 시험이 "사라진 것"으로 알리는 줄의 뜻:
  - `form post /x`: 그 주소로 보내는 폼.
  - `ctl /x input[text] name=n maxlength=20`: 그 폼 안의 입력과 속성. `button[submit] name=`은 보내는 단추.
  - `opt /x 이름 값`: 고르기의 값.
  - `link /x`: 링크.
  - `data qz-…`: 시험 데이터의 표지. 그 값을 가진 칸이 화면에 보여야 한다(`fixtures/sets/full.json`에서 찾으면 어느 칸인지 안다).
- 글 보존 시험이 알리는 글은 예전 화면 코드에 적혀 있던 한국어 조각이다. 화면 소스나 데몬이 주는 데이터에 그대로 있어야 한다. 화면의 짜임이 바뀌어 뜻이 없어진 글만 `tests/text-changes.ts`에 까닭과 함께 적는다(줄이거나 고쳐 쓴 글은 적지 않는다: 원래대로 되돌린다).
- 화면이 아직 자리 표시 글("아직 옮기는 중입니다")을 담고 있으면 그 화면의 시험은 건너뛴다(`skipped`). 다 옮기면 그 글을 지우고, 시험이 실제로 돌았는지(`passed`) 본다.
- 한 줄을 다른 화면으로 옮겼으면 `tests/parity-moves.ts`에 까닭과 함께 적는다. 없애는 것은 안 된다.

## 9. 부품이 모자랄 때

화면에서 때우지 않는다. 무엇이 왜 필요한지(어느 화면의 어느 대목, 지금 부품으로 안 되는 까닭) 적어서 알린다. `src/ui`와 `src/lib`는 한 사람이 고친다(모든 화면이 함께 쓰므로).
