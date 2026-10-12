---
title: 패널 디자인
description: 무엇에 어떤 요소를 쓰는지, 패널이 Agentty의 일부처럼 보이게 하는 몇 가지 규칙, 그리고 그대로 시작할 수 있는 완성 화면.
---

플러그인은 직접 그리지 않습니다. 요소의 트리를 보내면 Agentty가 앱과 같은 색·글꼴·간격으로 그립니다. 그래서 어떤 플러그인이든 Agentty만큼 보기 좋을 수 있습니다. 방법은 요소를 꾸미는 것이 아니라 알맞은 요소를 고르는 것입니다.

> [!TIP]
> **플러그인** 페이지에서 **UI Gallery**를 설치하세요(Agentty에 포함되어 있습니다). 모든 요소를 실제로 보여 주고 각 요소 아래에 그 요소를 그린 코드도 보여 줍니다. 이 페이지의 레시피도 직접 눌러 볼 수 있는 화면으로 들어 있습니다.

## 용도에 맞는 요소 고르기

| 보여 줄 것 | 쓸 요소 | 쓰지 말 것 |
|---|---|---|
| 사용자가 고르는 항목(노트, 작업, 세션) | `list` — 아이콘, 제목, 부제, 상세, 마우스를 올리면 나오는 버튼 | 버튼을 세로로 나열 |
| 같은 필드를 가진 기록(실행, 파일, 요청) | `table` — 머리글, 정렬된 열, 클릭하면 행 선택 | 필드를 부제에 이어 붙인 list |
| 한 항목의 세부 정보 | `card` 안의 `keyValue` | 콜론을 넣은 `text` 여러 줄 |
| 중요한 숫자 | `stat`, 여러 개면 `grid`로 | `title` 텍스트 |
| 함께 묶이는 것 | `card`(떠 있는 상자) 또는 `section`(조용한 제목) | 모든 사이에 `divider` |
| 한 플러그인의 여러 화면 | `tabs` — 선택된 탭의 내용만 보냅니다 | 맨 위의 `choice` |
| 진행 정도 | `progress`, 알 수 없으면 `spinner` | 텍스트로 쓴 퍼센트 |
| 힌트, 경고, 실패 원인 | `tone`을 준 `callout` | 문단 전체에 `error` 스타일 `text` |
| 설정 | `section` 안의 `input`, `select`, `checkbox`, `toggle` | 정해진 선택지를 자유 입력으로 |
| 코드, 명령, 출력 | `language`를 준 `code` | 여러 줄에 `code` 스타일 `text` |
| 자동화가 거치는 단계 | `flow` | 번호 매긴 목록 |

## 보기 좋게 만드는 규칙

1. **화면마다 primary 버튼은 하나.** 사용자가 하러 온 바로 그 일입니다. 나머지는 `secondary`, 취소 같은 것은 `ghost`, `danger`는 되돌릴 수 없는 일에만 씁니다.
2. **묶고, 간격은 맡기기.** 관련된 요소는 `card`나 `section`에 넣고 간격은 Agentty에 맡기세요. 간격은 `gap` 하나뿐이고 거의 모든 곳에서 `medium`이 맞습니다.
3. **tone에는 뜻이 있습니다.** `success`, `warning`, `error`는 상태를, `info`는 주목할 것을 말합니다. 장식으로 쓰면 아무 뜻도 전하지 못합니다.
4. **모든 목록에는 빈 상태가, 모든 호출에는 오류 상태가 있어야 합니다.** `list`와 `table`에는 다음에 할 일을 알려 주는 `empty` 문구를 주고, 실패는 다시 시도할 방법과 함께 `callout`으로 보여 주세요.
5. **일이 진행 중임을 보여 주기.** 무언가 시작되는 즉시 `spinner`나 `progress`를 보여 주세요. 클릭 후 아무것도 바뀌지 않는 패널은 고장 난 것처럼 보입니다.
6. **짧은 말, 문장형 대소문자.** "탭에서 열기"처럼 짧게. 자세한 내용은 제목이 아니라 부제나 `detail`에 넣으세요.
7. **패널은 좁습니다.** 시작 폭이 360px입니다. `grid`는 2열이면 충분하고, `table`은 3~4열까지가 적당합니다. `row`는 텍스트와 입력 칸이 폭을 나눠 쓰고 버튼은 자기 크기를 유지합니다.
8. **사용자의 언어로.** `context.language`는 `en`, `ko`, `ja`, `zh` 중 하나입니다. 문구는 표로 관리하세요.

## 레시피

그대로 복사해 쓸 수 있는 완성 화면입니다. 같은 코드가 UI Gallery의 **레시피**에 있습니다.

### 목록과 상세

```js
ui.column([
  ui.row([ui.input('search', { placeholder: 'Search tasks' }), ui.button('new', 'New', { variant: 'primary', icon: 'plus' })]),
  ui.list('tasks', tasks.map((t) => ({ id: t.id, title: t.title, subtitle: t.agent, icon: t.done ? 'circle-check' : 'loader-circle', tone: t.done ? 'success' : 'info' })),
    { empty: 'No tasks yet. Press New to start one.' }),
  picked && ui.card([
    ui.keyValue([
      { label: 'Status', value: picked.done ? 'Done' : 'Working', tone: picked.done ? 'success' : 'info' },
      { label: 'Branch', value: picked.branch, mono: true },
    ]),
    ui.row([ui.button('open', 'Open in a tab', { icon: 'external-link' }), ui.button('delete', 'Delete', { variant: 'ghost' })], { gap: 'small' }),
  ], { title: picked.title, icon: 'file-text' }),
]);
```

### 설정 폼

```js
ui.column([
  ui.section('Connection', [
    ui.input('url', { placeholder: 'https://api.example.com', value: settings.url }),
    ui.select('env', [{ value: 'dev', label: 'Development' }, { value: 'prod', label: 'Production' }], settings.env),
  ]),
  ui.section('Behavior', [
    ui.toggle('watch', 'Watch for changes', settings.watch),
    ui.checkbox('confirm', 'Ask before sending prompts', settings.confirm, { description: 'Shows the prompt to the user first.' }),
  ]),
  ui.row([ui.button('save', 'Save', { variant: 'primary' }), ui.button('reset', 'Reset', { variant: 'ghost' })], { gap: 'small' }),
]);
```

### 대시보드

```js
ui.column([
  ui.grid([
    ui.stat('Open PRs', String(prs.length), { detail: '+2 today', tone: 'info', icon: 'git-pull-request' }),
    ui.stat('CI', ci.ok ? 'passing' : 'failing', { tone: ci.ok ? 'success' : 'error', icon: ci.ok ? 'circle-check' : 'circle-x' }),
  ]),
  ui.section('Recent runs', [
    ui.table('runs', [{ label: 'Workflow', grow: 3 }, { label: 'Result', grow: 2 }, { label: 'Time', align: 'end' }],
      runs.map((r) => ({ id: r.id, cells: [r.name, r.result, r.duration], tone: r.ok ? 'success' : 'error' }))),
  ]),
]);
```

### 실행 중인 작업

```js
ui.card([
  ui.progress(done / total, { label: `Step ${done + 1} of ${total} · ${step}` }),
  ui.code(output, { language: 'sh' }),
  ui.row([ui.button('stop', 'Stop', { variant: 'danger', icon: 'square' })]),
], { title: 'Release check', subtitle: `started ${since}`, icon: 'rocket', tone: 'info' });
```

### 빈 상태와 오류

```js
ui.column([
  ui.list('notes', [], { empty: 'No notes yet. Create one to get started.' }),
  ui.callout('Check the token in Settings and try again.', { title: 'Could not sign in', tone: 'error' }),
  ui.row([ui.button('retry', 'Try again', { variant: 'primary', icon: 'refresh-cw' })]),
]);
```

## 요소

모든 빌더의 옵션과 이벤트는 [Node.js SDK](/docs/plugin-sdk#패널)에, 각 요소가 보내는 JSON은 [프로토콜](/docs/plugin-protocol#ui-트리)에 있습니다. 요약하면:

| 요소 | 이벤트 |
|---|---|
| `column`, `row`, `grid`, `section`, `card`, `divider` | |
| `tabs` | 탭 id와 함께 `change` |
| `text`, `badge`, `keyValue`, `stat`, `code`, `callout`, `progress`, `spinner` | |
| `list` | `item`과 함께 `select`, 행 버튼은 `item`·`action`과 함께 `action` |
| `table` | `item`(행 id)과 함께 `select` |
| `flow` | `item`(단계 id)과 함께 `select` |
| `button` | `click` |
| `input` | 입력이 멈추면 `change`, Enter에 `submit` |
| `select`, `choice` | 옵션 값과 함께 `change` |
| `checkbox`, `toggle` | 새 boolean과 함께 `change` |
| `popover` | `close` |

`card`, `grid`, `tabs`, `table`, `keyValue`, `stat`, `progress`, `callout`, `select`, `checkbox`, `code`는 API 버전 4입니다. 쓸 때는 매니페스트에 `"apiVersion": 4`를 적으세요. 그러면 이전 버전 Agentty는 그릴 수 없는 패널을 보여 주는 대신 업데이트하라고 안내합니다. 두 버전 모두에서 동작하려는 플러그인은 `host/info`의 `uiFeatures`로 실행 중인 Agentty가 그릴 수 있는 요소를 확인할 수 있습니다.

## 다음

- [Node.js SDK](/docs/plugin-sdk) · [Rust와 WebAssembly](/docs/plugin-rust)
- [플러그인 프로토콜](/docs/plugin-protocol) — JSON으로 된 UI 트리와 그 제한
