---
title: 플러그인 프로토콜
description: SDK 뒤에서 오가는 JSON-RPC 와이어 포맷, WebAssembly 모듈 ABI, 그리고 모든 메시지와 제한, 오류 코드.
---

SDK 없이 플러그인을 작성하기 위한 와이어 포맷입니다. [Node.js SDK](/docs/plugin-sdk)와 [Rust SDK](/docs/plugin-rust)가 전부 감싸 주며, 어느 쪽이든 [빠른 시작](/docs/plugin-quickstart)을 먼저 읽는 편이 좋습니다.

API **버전 1**은 이 페이지에서 `host/timer`와 `pane/status`를 뺀 전부입니다. 그 둘은 **버전 2**입니다.

## 전송 방식

Agentty는 플러그인 폴더를 작업 디렉터리로 삼아 프로그램을 시작합니다.

| `runtime` | 실행 명령 |
|---|---|
| `node` | `node <main>` — 로그인 셸 `PATH`, Homebrew, Volta, nvm의 Node.js |
| `python` | `python3 <main>` |
| `executable` | `<main>` |
| `wasm` | 없음 — `<main>`이 Agentty가 직접 실행하는 WebAssembly 모듈. [WebAssembly 플러그인](#webassembly-플러그인) 참고 |

메시지는 [JSON-RPC 2.0](https://www.jsonrpc.org/specification) 객체이며 **한 줄에 하나**, UTF-8로, stdin(Agentty → 플러그인)과 stdout(플러그인 → Agentty)을 통해 오갑니다. 16MB가 넘는 줄은 거부됩니다. stdout의 JSON이 아닌 내용은 로그에 남고 무시되며, stderr는 플러그인 로그로 갑니다.

stdin이 닫히거나 `shutdown`이 오면 종료하세요. `shutdown` 후 1.5초가 지나도 살아 있으면 `SIGTERM`을, 다시 1.5초 뒤에 `SIGKILL`을 받습니다. Agentty가 종료될 때는 둘 다 즉시 이어집니다.

## Agentty → 플러그인

| 메시지 | 종류 | `params` |
|---|---|---|
| `initialize` | 요청 — 응답 필요 | `{ apiVersion, agentty: { version }, plugin: { id, name, version, dir, dataDir }, language, context }` |
| `command/execute` | 알림 | `{ command, args, context }` |
| `panel/open` · `panel/close` | 알림 | `{ context }` |
| `ui/event` | 알림 | `{ element, event, value?, item?, action?, context }` |
| `context/changed` | 알림 | `{ context }` |
| `url/open` | 알림 | `{ path, query, url, context }` |
| `pane/status` | 알림 | `{ paneId, status, running, agent, title, cwd }` — 이 플러그인이 시작한 페인의 상태가 바뀜 (`workspace.read`, API 2) |
| `shutdown` | 알림 | `{}` |

`initialize`가 먼저 오고, 곧바로 플러그인을 시작시킨 것(명령, 패널 열기, 링크)이 뒤따릅니다. `initialize`에는 아무 결과나 응답하면 됩니다(예: `{}`).

## 플러그인 → Agentty

결과나 오류를 받으려면 요청(`id` 포함)으로, 필요 없으면 알림(`id` 없음)으로 보냅니다.

| 메서드 | 권한 | `params` | 결과 |
|---|---|---|---|
| `ui/setPanel` | | `{ tree }` | `null` |
| `ui/showPanel` | | `{}` | `null` |
| `ui/notify` | | `{ message, kind }` — `info`, `success`, `warning`, `error` | `null` |
| `ui/setBadge` | | `{ text }`, 최대 8자 | `null` |
| `context/get` | | `{}` | 컨텍스트 |
| `host/info` | | `{}` | `{ version, apiVersion, language }` |
| `host/openUrl` | | `{ url }` — http/https | `null` |
| `host/copy` | | `{ text }` — 최대 100,000자 | `null` |
| `host/timer` | | `{ ms }` — API 2 | 시간이 지나면 `{ elapsedMs }` |
| `host/revealPath` | `workspace.read` | `{ path }` — 존재하는 절대 경로 | `null` |
| `prompt/inject` | `prompt.inject` | `{ text, title?, target?, paneId?, workspaceId?, agent?, cwd?, submit? }` | `{ status: "asked" }` 또는 `{ status: "sent", paneId }` |
| `terminal/send` | `terminal.write` | `{ paneId?, text, submit? }` — `paneId` 없으면 포커스된 페인 | `{ paneId }` |
| `session/get` | `session.read` | `{ paneId?, maxTurns? }` — 기본 200, 최대 2000 | `{ paneId, agent, sessionId, title, cwd, status, turnCount, turns }` |
| `workspace/list` | `workspace.read` | `{}` | `[{ id, name, cwd, active, panes }]` |
| `net/fetch` | `net.request` | `{ url, method?, headers?, body?, timeoutMs?, proxy? }` | `{ status, statusText, url, headers, body, truncated, binary, bytes, durationMs }` |
| `storage/get` | | `{ key }` | `{ key, value }` — 값이 없으면 `value`는 null |
| `storage/set` | | `{ key, value }` — null이면 삭제 | `null` |
| `storage/keys` | | `{}` | `[key]` |

700ms보다 빠르게 도착하는 `ui/notify`는 정상 응답 후 폐기되며, 초당 240건을 넘게 보내는 플러그인은 중단됩니다. 컨텍스트 필드는 플러그인의 권한에 따라 제한됩니다.

### 네트워크에 닿기

`net/fetch`는 플러그인이 네트워크에 닿는 유일한 방법입니다. Agentty가 거는 제한 — 메서드, 헤더, 크기, 타임아웃, 리다이렉트 — 은 [권한](/docs/plugin-permissions#net-request로-할-수-있는-일)에 있습니다. 요약하면, 요청에는 여러분의 것이 아무것도 실리지 않습니다. 쿠키도 저장된 자격 증명도 없고, 플러그인이 직접 넣은 것만 갑니다.

### 기다리기, 그리고 에이전트가 끝난 것을 듣기

`host/timer`는 플러그인이 기다리는 방법입니다. 시간이 지나면 응답되는 요청이고, 최소 100ms, 최대 1시간, 동시에 8개까지입니다. 모듈은 메시지를 처리하는 동안에만 실행되므로, 나중의 어떤 시점으로 돌아오는 수단은 이것이 전부입니다. 플러그인이 얻는 것은 그 응답뿐이라서 백그라운드 실행 수단이 되지 않습니다.

`pane/status`는 플러그인이 시작한 에이전트가 끝난 것을 듣는 방법입니다. 플러그인은 `prompt/inject`의 응답(`{ status: "sent", paneId }`)에서 페인 id를 알게 되고, Agentty는 어느 플러그인이 어느 페인을 시작했는지 기억했다가 그 플러그인에게만 상태 변화를 알립니다. `working`, `idle`, `finished`, `permission`, `question`, `interrupted`, `exited`, 그리고 마지막에 한 번 `closed`입니다. 동시에 최대 32개 페인까지 추적합니다.

이용자가 직접 보낸 프롬프트도 추적됩니다. `target: "ask"`는 아직 페인이 없으므로 페인 id 없이 `{ status: "asked" }`로 응답하지만, 이용자가 고른 세션도 똑같이 지켜봅니다. 그 세션의 첫 `pane/status`가 플러그인이 어느 페인이 되었는지 알게 되는 지점입니다. 질문이 여럿 열려 있는 플러그인은 프롬프트에 붙인 `title`로 구분합니다.

화면이 그려지고 있든 아니든 상태는 전달됩니다. 다른 창에 가려진 창이나 잠긴 화면의 창은 그려지지 않는데, 에이전트를 기다리는 플러그인이 이용자가 돌아오기를 기다리고 있어서는 안 되기 때문입니다. [AgentOS 플러그인](/docs/plugin-agentos)이 이 둘 위에 만들어져 있습니다.

### 기억하기

`storage/*`는 플러그인이 실행 사이에 기억하는 수단입니다. 자기 폴더의 JSON 문서 하나(`<데이터 폴더>/plugin-data/<plugin>/storage.json`, `0600`으로 생성)를 키로 읽고 씁니다. 키는 소문자·숫자·`.`·`-`·`_`이며 최대 64개, 총 1MB입니다. 프로세스로 실행되는 플러그인은 자기 파일을 쓸 수 있지만, WebAssembly 플러그인에는 파일이 없으므로 무언가를 남기는 수단은 이것뿐입니다.

### 오류 코드

| 코드 | 의미 |
|---|---|
| `-32601` | 알 수 없는 메서드 |
| `-32602` | 잘못된 파라미터 — 잘못된 UI 트리, 존재하지 않는 페인 등 |
| `-32001` | 권한 없음, 또는 링크가 도달해 차단된 상태 — 재시작할 때까지 |
| `-32002` | 사용 불가 — 열린 창이 없음, 세션이 아직 없음 |

## 주고받는 예시

```
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"apiVersion":1,"plugin":{"id":"hello"},"language":"en","context":{}}}
→ {"jsonrpc":"2.0","method":"panel/open","params":{"context":{}}}
← {"jsonrpc":"2.0","id":1,"result":{}}
← {"jsonrpc":"2.0","id":1,"method":"ui/setPanel","params":{"tree":{"type":"column","children":[{"type":"button","id":"go","label":"Go"}]}}}
→ {"jsonrpc":"2.0","id":1,"result":null}
→ {"jsonrpc":"2.0","method":"ui/event","params":{"element":"go","event":"click","context":{}}}
← {"jsonrpc":"2.0","id":2,"method":"prompt/inject","params":{"text":"Hello","target":"ask"}}
→ {"jsonrpc":"2.0","id":2,"result":{"status":"asked"}}
```

`→`는 Agentty에서 플러그인으로, `←`는 플러그인에서 Agentty로입니다. 요청 id는 방향별로 셉니다.

## WebAssembly 플러그인

`"runtime": "wasm"`으로 지정하면 `main`은 Agentty가 자기 안에서 인터프리터로 실행하는 `.wasm` 모듈이 됩니다. 파일 하나가 macOS·Windows·Linux에서 그대로 동작합니다.

모듈은 Agentty가 건네준 함수만 호출할 수 있습니다. 파일도, 소켓도, 환경 변수도, 프로세스도 없고, 카운터 말고는 시계도 없습니다. WebAssembly 플러그인은 어떻게 작성했든 `~/.agentty`도, 이용자의 프로젝트도, 자격 증명도 읽을 수 없습니다. Agentty에 원하는 것은 프로세스 플러그인이 stdout에 쓰는 것과 똑같은 메시지로 요청하고, 매니페스트의 권한도 똑같이 검사됩니다.

모듈이 export하는 것:

| Export | 의미 |
|---|---|
| `memory` | 선형 메모리 — Rust나 C 모듈의 표준 export |
| `agentty_alloc(len: i32) -> i32` | Agentty가 메시지를 써 넣을 `len` 바이트 버퍼 |
| `agentty_on_message(ptr: i32, len: i32)` | Agentty가 보낸 UTF-8 JSON 메시지 하나 |

그리고 `agentty`라는 모듈에서 import하는 것:

| Import | 의미 |
|---|---|
| `send(ptr: i32, len: i32)` | Agentty로 보내는 UTF-8 JSON 메시지 하나 |
| `log(ptr: i32, len: i32)` | 플러그인 로그에 남길 한 줄 |
| `now_ms() -> i64` | Unix epoch 기준 밀리초 |

메시지는 stdio로 오가는 것과 같은 JSON-RPC 객체이며, 호출마다 하나씩 줄바꿈 없이 전달됩니다. **이외의 것을 import하는 모듈은 로드되지 않습니다.** 64MB가 넘는 모듈도 거부되고, 메모리는 64MB로 제한되며, 메시지마다 작업량 예산이 있습니다. 돌아오지 않는 플러그인은 "제시간에 끝내지 못함"으로 중단되고, 메시지 하나를 처리하는 동안 256개가 넘는 메시지를 보내는 플러그인도 중단됩니다. `send`와 `log`를 합쳐서 세므로, 로그만 찍는 루프도 공짜가 아닙니다.

[Rust SDK](/docs/plugin-rust)가 이 모든 것을 감춰 줍니다.

## UI 트리

모든 노드는 `type`을 가진 객체입니다.

```
column   { children, gap? }                gap: none | small | medium | large
row      { children, gap?, wrap? }
section  { title, children }
text     { text, style? }                  style: body | title | muted | small | code | error | success
button   { id, label, icon?, variant?, disabled? }   variant: primary | secondary | ghost | danger
input    { id, placeholder?, value?, rows? }
         rows > 1: 그만큼의 줄을 가진 텍스트 영역(최대 24). Enter는 줄을
         바꾸고, 붙여넣기는 줄바꿈을 유지합니다
list     { id, items, empty? }
         items: [{ id, title, subtitle?, detail?, icon?, tone?, actions?: [{ id, label?, icon?, tooltip? }] }]
choice   { id, options: [{ value, label }], value? }
toggle   { id, label, value? }
badge    { text, tone? }                   tone: neutral | info | success | warning | error
spinner  { text? }
divider  {}
```

이벤트: `button`은 `click`, `input`은 `value`와 함께 `change`·`submit`, `list`는 `item`과 함께 `select`(행 버튼은 `item`·`action`과 함께 `action`), `choice`는 선택한 값과 함께 `change`, `toggle`은 새 불리언과 함께 `change`를 보냅니다.

리스트 항목의 `tone`은 아이콘 색을 정하며 `badge`와 같은 값을 씁니다.

## Agentty 없이 테스트하기

`node`·`python`·`executable` 플러그인은 stdin을 읽고 stdout에 쓰는 평범한 프로그램이므로 테스트에서 직접 구동할 수 있습니다. `initialize` 요청을 쓰고, 확인하고 싶은 알림을 이어서 보낸 뒤, 플러그인이 돌려주는 JSON을 검증하면 됩니다.

`wasm` 모듈도 방식은 같습니다. 위의 import 세 개를 제공할 수 있는 WebAssembly 런타임이면 무엇으로든 구동할 수 있습니다.
