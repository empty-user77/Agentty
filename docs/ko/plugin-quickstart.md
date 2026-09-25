---
title: 플러그인 빠른 시작
description: AI로, Rust로, 또는 JavaScript로 동작하는 Agentty 플러그인을 만들고 설치하기.
---

플러그인은 매니페스트와 프로그램이 든 폴더입니다. 터미널 옆에 **패널**을 두고, 에이전트 페인 위에 **버튼**을 추가하고, **명령 팔레트**에 항목을 넣고, 텍스트를 **프롬프트**로 에이전트에 넘길 수 있습니다.

종류가 둘이고, 먼저 고르는 편이 좋습니다.

| | |
|---|---|
| **WebAssembly** (`runtime: "wasm"`) | `.wasm` 파일 하나. 보통 Rust로 빌드합니다. Agentty가 직접 실행하므로 프로토콜이 허용한 것만 건드리고, 이용자 컴퓨터에 설치할 것이 없습니다. [마켓플레이스](/docs/plugin-publishing)가 받는 유일한 형태입니다. |
| **프로그램** (`node`, `python`, `executable`) | 이용자 권한으로 실행되며, 이용자가 실행하는 다른 프로그램과 같은 접근 권한을 가집니다. 작성이 빠르고, 이용자가 직접 고른 폴더나 Git 저장소에서 설치합니다. |

## AI로 만들기

대부분의 플러그인은 이렇게 만들어집니다. 액티비티 바의 퍼즐 아이콘으로 **플러그인**을 열고, *직접 만들기* 아래에 이름과 무엇을 하는 플러그인인지 적은 뒤 **Claude Code로 생성하고 빌드**를 누릅니다.

Agentty가 템플릿으로 플러그인을 만들고 — SDK, 타입 정의, 개발자 가이드, 지시사항이 담긴 `CLAUDE.md`/`AGENTS.md` — 그 폴더에서 Claude Code를 엽니다. **AI 프롬프트 복사**를 누르면 같은 프롬프트를 다른 에이전트에 쓸 수 있습니다.

### 에이전트에게 무엇을 말할까

템플릿이 이미 가이드를 품고 있으므로, 쓸모 있는 프롬프트는 플랫폼 설명이 아니라 만들려는 플러그인 자체입니다.

```text
먼저 PLUGIN_GUIDE.md와 agentty-plugin.d.ts를 읽어.

포커스된 페인의 폴더에 있는 Git 브랜치를 나열하는 패널을 만들어.
각 행에는 브랜치 이름과, main보다 얼마나 앞서거나 뒤처졌는지 표시해.
행을 클릭하면 그 브랜치에서 무엇이 바뀌었는지 에이전트에게 요약을 요청해.

권한은 prompt.inject와 workspace.read만 요청하고, 그 외에는 요청하지 마.
```

그다음 코드를 검사하게 하고(`node --check main.mjs`, 또는 `cargo build --release --target wasm32-unknown-unknown`), Agentty의 플러그인 카드에서 **재시작**을 눌러 반영합니다. 오류는 같은 카드의 **로그**에 있습니다.

> [!TIP]
> 템플릿 폴더 밖에서 작업하는 에이전트에게 프롬프트를 쓴다면 [매니페스트 레퍼런스](/docs/plugin-manifest), [UI 트리](/docs/plugin-protocol#ui-트리), [프로토콜](/docs/plugin-protocol)을 가리켜 주세요. 이 세 페이지가 전부입니다.

## 직접 만들기 — Rust

```
hello/
├── agentty-plugin.json   매니페스트
├── hello.wasm            컴파일된 모듈
└── src/lib.rs            소스
```

```rust
use agentty_plugin::{export_plugin, ui, Host, Plugin, UiEvent};

#[derive(Default)]
struct Hello {
    clicks: u32,
}

impl Plugin for Hello {
    fn panel_open(&mut self, host: &Host) {
        host.set_panel(ui::column(vec![
            ui::text(format!("Clicked {} times", self.clicks)),
            ui::button("go", "Click me"),
        ]));
    }

    fn ui_event(&mut self, host: &Host, event: UiEvent) {
        if event.element == "go" {
            self.clicks += 1;
            self.panel_open(host);
        }
    }
}

export_plugin!(Hello);
```

```json
{
  "id": "hello",
  "name": "Hello",
  "version": "0.1.0",
  "runtime": "wasm",
  "main": "hello.wasm",
  "contributes": { "panel": { "title": "Hello", "surface": "sidebar" } }
}
```

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/hello.wasm hello.wasm
```

SDK 전체는 [Rust와 WebAssembly](/docs/plugin-rust)에 있습니다.

## 직접 만들기 — JavaScript

```
~/.agentty/plugins/hello/
├── agentty-plugin.json   매니페스트
├── main.mjs              플러그인 프로그램
└── agentty-plugin.mjs    SDK
```

SDK는 의존성 없는 파일 하나입니다. 플러그인 페이지에서 **개발자 가이드**를 누르면 Agentty가 타입과 문서까지 `~/.agentty/plugins/.sdk/`에 풀어 놓습니다. 거기서 `agentty-plugin.mjs`를 `main.mjs` 옆으로 복사하세요.

```json
{
  "id": "hello",
  "name": "Hello",
  "version": "0.1.0",
  "main": "main.mjs",
  "permissions": ["prompt.inject"],
  "contributes": {
    "panel": { "title": "Hello", "icon": "sparkles" },
    "commands": [{ "id": "hello.explain", "title": "Hello: Explain this folder", "icon": "bot" }]
  }
}
```

```js
import { createPlugin, ui } from './agentty-plugin.mjs';

const plugin = createPlugin();

plugin
  .onPanelOpen((context) =>
    plugin.setPanel(
      ui.column([
        ui.text('Hello', 'title'),
        ui.text(context.pane ? `You are in ${context.pane.cwd}` : 'No terminal focused', 'muted'),
        ui.button('explain', 'Explain this folder', { icon: 'bot', variant: 'primary' }),
      ]),
    ),
  )
  .onEvent('explain', (_event, context) => explain(context))
  .command('hello.explain', ({ context }) => explain(context))
  .start();

function explain(context) {
  return plugin.injectPrompt({ text: 'Give me a short tour of this project.', cwd: context.pane?.cwd, target: 'ask' });
}
```

> [!WARNING]
> stdout에 절대 쓰지 마세요(`console.log`). 프로토콜이 지나가는 통로입니다. 로그는 `plugin.log()`나 `console.error()`로 남기세요.

나머지는 [Node.js SDK](/docs/plugin-sdk)에 있습니다.

## 설치하기

**플러그인 → 폴더에서 설치…** 로 폴더를 고르거나, **개발용 폴더 연결…** 로 있는 자리에서 실행합니다. 그다음 **새로 고침**하면(또는 페이지를 다시 열면) 플러그인이 설치된 것으로 나타납니다. 패널 버튼은 `surface`가 가리키는 자리에, 명령은 팔레트(⇧⌘P)에 나타납니다.

## 테스트하기

`node`·`python`·`executable` 플러그인은 stdin을 읽고 stdout에 쓰는 프로그램이므로, Agentty 없이도 직접 구동할 수 있습니다.

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"plugin":{"id":"hello","dataDir":"/tmp"},"context":{}}}' \
  '{"jsonrpc":"2.0","method":"panel/open","params":{"context":{}}}' \
  | node main.mjs
```

`initialize`에 응답한 뒤 `ui/setPanel` 요청을 보내야 정상입니다.

## 다음으로

- [매니페스트 레퍼런스](/docs/plugin-manifest) — 모든 필드
- [Rust와 WebAssembly](/docs/plugin-rust) · [Node.js SDK](/docs/plugin-sdk)
- [AgentOS 플러그인](/docs/plugin-agentos) — 에이전트를 통해 일을 진행하기
- [권한](/docs/plugin-permissions) · [배포하기](/docs/plugin-publishing)
