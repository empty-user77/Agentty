---
title: Node.js SDK
description: 핸들러, 호출, 패널 UI 빌더, 컨텍스트 객체 — agentty-plugin.mjs의 전체 기능.
---

`agentty-plugin.mjs`는 의존성이 없는 파일 하나입니다. 타입 정의는 같은 위치의 `agentty-plugin.d.ts`에 있습니다. 플러그인 페이지에서 **개발자 가이드**를 누르면 둘 다 `~/.agentty/plugins/.sdk/`에 풀립니다.

> [!NOTE]
> 이렇게 만든 플러그인은 이용자 컴퓨터에서 프로그램으로 실행되며 `PATH`에 Node.js 18 이상이 필요하고, 폴더나 Git 저장소에서 설치됩니다. [마켓플레이스](/docs/plugin-publishing)는 WebAssembly 모듈만 받습니다. 그쪽은 [Rust와 WebAssembly](/docs/plugin-rust)를 보세요.

```js
import { createPlugin, ui } from './agentty-plugin.mjs';

const plugin = createPlugin();
// 핸들러 등록…
plugin.start();
```

핸들러는 모두 `start()` 호출 전에 등록해야 합니다.

## 핸들러

모든 핸들러는 async일 수 있습니다. 오류는 로그에 남고 이용자에게 알림으로 표시됩니다.

| 핸들러 | 호출 시점 |
|---|---|
| `onActivate(info => …)` | 플러그인이 시작됨. `info`에 `plugin.dataDir`, `language`, `context` |
| `command(id, ({ context, args }) => …)` | 명령 팔레트에서 명령 실행 |
| `onPanelOpen(context => …)` | 패널이 보이게 됨 — 여기서 그립니다 |
| `onPanelClose(context => …)` | 패널이 숨겨짐 |
| `onEvent(elementId, (event, context) => …)` | 해당 id의 UI 요소가 사용됨 |
| `onAnyEvent((event, context) => …)` | `onEvent`가 처리하지 않은 모든 UI 이벤트 |
| `onContextChange(context => …)` | 포커스된 페인, 그 상태, 폴더가 바뀜 |
| `onUrl(path, ({ path, query, url }) => …)` | `agentty://plugin/<id>/<path>?…`가 열림 |
| `onShutdown(() => …)` | Agentty가 플러그인을 중단시킴 |

## 호출

모든 호출은 프라미스를 반환합니다.

| 호출 | 권한 |
|---|---|
| `setPanel(tree)` — 패널 내용 교체 | |
| `showPanel()` — 이 플러그인의 패널 열기 | |
| `notify(message, kind)` — `info`, `success`, `warning`, `error` | |
| `setBadge(text)` — 탭 영역 버튼에 최대 8자 | |
| `getContext()` | |
| `openUrl(url)` — http/https | |
| `copy(text)` — 클립보드에 복사 | |
| `revealPath(path)` — 파일 관리자에서 보기 | `workspace.read` |
| `injectPrompt(request)` | `prompt.inject` |
| `sendToTerminal({ paneId, text, submit })` | `terminal.write` |
| `getSession({ paneId, maxTurns })` | `session.read` |
| `listWorkspaces()` | `workspace.read` |
| `fetch(request)` — HTTP 요청 | `net.request` |
| `log(...)` — 플러그인 로그(stderr)에 기록 | |

프로토콜에는 `storage/get`·`storage/set`·`storage/keys`(플러그인 자기 폴더의 JSON 문서. 권한 불필요)와, API 버전 2부터 `host/timer`·`pane/status`도 있습니다. Node 플러그인은 자기 파일을 써도 되지만 storage는 두 종류 모두에서 똑같이 동작합니다. [프로토콜](/docs/plugin-protocol)을 참고하세요.

`plugin.info`에는 `initialize` 데이터가, `plugin.context`에는 최신 컨텍스트가 들어 있습니다.

## 패널

패널은 폭 360px이고 세로로 스크롤됩니다. 플러그인이 트리로 설명하면 Agentty가 네이티브로 그리므로 앱과 이질감이 없고 웹뷰가 필요 없습니다. 무언가 바뀔 때마다 새 트리를 보내면 됩니다. 텍스트 필드는 다른 `value`를 보내지 않는 한 이용자가 입력한 값을 유지합니다.

| 빌더 | 요소 | 이벤트 |
|---|---|---|
| `ui.column(children, { gap })` / `ui.row(children, { gap, wrap })` | 레이아웃. `gap`: `none`, `small`, `medium`, `large` | |
| `ui.section(title, children)` | 제목이 있는 묶음 | |
| `ui.text(text, style)` | `body`, `title`, `muted`, `small`, `code`, `error`, `success` | |
| `ui.button(id, label, { icon, variant, disabled })` | `primary`, `secondary`, `ghost`, `danger` | `click` |
| `ui.input(id, { placeholder, value, rows })` | 한 줄 입력, `rows`가 1보다 크면 그만큼의 텍스트 영역(최대 24) | 입력이 멈추면 `change`, Enter에 `submit`. `event.value`가 텍스트 |
| `ui.list(id, items, { empty })` | 행 `{ id, title, subtitle, detail, icon, tone, actions }` | `event.item`과 함께 `select`, 행 버튼은 `event.item`·`event.action`과 함께 `action` |
| `ui.choice(id, [{ value, label }], value)` | 분할 선택 | 값과 함께 `change` |
| `ui.toggle(id, label, value)` | 스위치 | 새 불리언과 함께 `change` |
| `ui.badge(text, tone)` | `neutral`, `info`, `success`, `warning`, `error` | |
| `ui.spinner(text)` | | |
| `ui.divider()` | | |

null과 false인 자식은 건너뛰므로 `조건 && ui.text('…')`가 그대로 동작합니다.

```js
plugin.onPanelOpen(async (context) => {
  const notes = await search('');
  plugin.setPanel(
    ui.column([
      ui.input('q', { placeholder: 'Search notes' }),
      ui.list('notes', notes.map((n) => ({
        id: n.path,
        title: n.title,
        subtitle: n.folder,
        icon: 'notebook',
        actions: [{ id: 'insert', icon: 'send', tooltip: 'Insert into the focused pane' }],
      })), { empty: 'No notes yet' }),
    ]),
  );
});

plugin.onEvent('notes', (event, context) => {
  if (event.event === 'action' && event.action === 'insert') {
    return plugin.injectPrompt({ text: read(event.item), target: 'ask' });
  }
});
```

> [!NOTE]
> 제한: 요소 2,000개, 깊이 12단계, 문자열당 20,000자. `choice`의 옵션과 리스트 항목의 버튼도 요소로 셉니다. 패널은 최소 50ms 간격으로 다시 그려지고 알림은 최소 700ms 간격입니다. 초당 240건을 넘게 보내는 플러그인은 폭주로 간주되어 중단됩니다.

## 컨텍스트

모든 명령·이벤트·패널 호출에는 포커스된 창의 컨텍스트가 실립니다.

```json
{
  "workspace": { "id": 3, "name": "agentty", "cwd": "/Users/me/agentty", "active": true },
  "pane": {
    "id": 12,
    "kind": "claude",
    "tool": "claude",
    "title": "Claude Code",
    "cwd": "/Users/me/agentty",
    "sessionId": "…",
    "status": "idle",
    "running": true
  },
  "language": "ko"
}
```

플러그인이 보는 범위는 선언한 권한에 따라 달라집니다. 폴더와 이름 필드(`workspace.cwd`, `workspace.name`, `pane.cwd`, `pane.title`)에는 `workspace.read`가, `pane.sessionId`에는 `session.read`가 필요합니다. 권한이 없으면 id, `kind`, `tool`, `status`, `running`, 언어만 남습니다. 어느 페인이 포커스됐는지는 알 수 있어도 이용자가 어디서 작업하는지는 알 수 없습니다.

`kind`는 `claude`, `codex`, `shell`입니다. 다른 에이전트 CLI는 `shell` 페인에서 실행되며 `tool`이 이름을 알려줍니다.

| `status` | 의미 |
|---|---|
| `idle` | 입력 대기 |
| `working` | 도구 실행 중 |
| `thinking` | 턴이 열린 상태, 도구 호출 사이 |
| `finished` | 턴 종료 |
| `permission` | 권한을 묻는 중 |
| `question` | 이용자에게 질문 중 |
| `interrupted` | 이용자가 중단함 |
| `shell` | 일반 셸 |
| `exited` | 프로그램 종료 |

방해하지 않을 판단을 할 때 `working`과 `thinking`은 같게 취급하세요.

## 프롬프트 보내기

```js
await plugin.injectPrompt({
  text: 'Continue the release checklist.',
  title: 'Release',          // 새 세션의 워크스페이스 이름이자 다이얼로그 제목
  target: 'ask',             // ask | active | newWorkspace | newTab | pane | workspace
  agent: 'claude',           // 새 세션용 claude | codex | shell
  cwd: '/Users/me/project',  // 새 세션의 폴더
  submit: true,              // Enter 누르기 (에이전트만)
});
```

- `ask`(기본값)는 **보낼 곳…**을 띄워 이용자가 목적지를 고르게 합니다.
- `active`는 포커스된 페인에, `pane`은 `paneId`에, `workspace`는 `workspaceId`에 입력하고, `newWorkspace`·`newTab`은 그 프롬프트로 새 세션을 시작합니다.
- 터미널에는 언제나 입력만 됩니다. `injectPrompt`는 Enter를 누르지 않습니다.
- 60,000바이트가 넘는 프롬프트는 `~/.agentty/prompts/`에 저장되고 에이전트에게 그 파일을 읽으라고 전달됩니다.

다른 앱이나 링크에서 시작된 것이라면 `ask`를 쓰세요.

## 세션과 터미널

```js
const session = await plugin.getSession({ paneId: context.pane.id, maxTurns: 200 });
// { agent, sessionId, title, cwd, status, turnCount, turns: [{ role: 'user' | 'assistant', text }] }

await plugin.sendToTerminal({
  paneId: context.pane.id,
  text: 'Summarize what we did.',
  submit: true,
});
```

에이전트에 입력하기 전에 `pane.status`를 확인하세요. `working`, `permission`, `question`은 방해하면 안 됩니다.

## 다음으로

- [프로토콜](/docs/plugin-protocol) — SDK 없이 같은 기능 쓰기
- [권한](/docs/plugin-permissions)
