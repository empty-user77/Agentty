---
title: Rust와 WebAssembly
description: Rust로 플러그인을 만들어 어디서나 돌아가는 .wasm 파일 하나로 배포하고, 프로토콜이 허용한 것만 건드리게 하는 방법.
---

`"runtime": "wasm"`으로 지정하면 `main`은 Agentty가 **자기 안에서** 인터프리터로 실행하는 WebAssembly 모듈이 됩니다. 파일 하나가 macOS·Windows·Linux에서 그대로 동작하고, 이용자 컴퓨터에 설치할 것이 없습니다. Node.js도 Python도 필요 없습니다.

[마켓플레이스](/docs/plugin-publishing)가 등록을 받는 유일한 형태이기도 합니다. 그럴 수 있는 이유는 모듈이 프로토콜이 건네준 것만 건드릴 수 있기 때문입니다.

## 모듈이 닿을 수 있는 것

Agentty가 모듈에 넘기는 함수는 셋뿐입니다.

| `agentty` 모듈의 import | |
|---|---|
| `send(ptr, len)` | Agentty로 보내는 UTF-8 JSON 메시지 하나 |
| `log(ptr, len)` | 플러그인 로그에 남길 한 줄 |
| `now_ms() -> i64` | Unix epoch 기준 밀리초 |

파일도, 소켓도, 환경 변수도, 프로세스도 없고, 저 카운터 말고는 시계도 없습니다. WebAssembly 플러그인은 **어떻게 작성했든** `~/.agentty`도, 여러분의 프로젝트도, 자격 증명도 읽을 수 없습니다. 읽지 않겠다고 약속해서가 아니라, 그럴 함수를 애초에 받지 못했기 때문입니다. 이외의 것을 import하는 모듈은 아예 로드되지 않습니다.

나머지는 프로세스 플러그인이 stdout에 쓰는 것과 똑같은 JSON-RPC 메시지로 Agentty에 요청하고, `agentty-plugin.json`의 [권한](/docs/plugin-permissions)도 똑같이 검사됩니다.

> [!NOTE]
> `runtime`이 `node`·`python`·`executable`인 플러그인은 정반대입니다. 이용자 권한으로 실행되며, 이용자가 직접 실행하는 다른 프로그램과 같은 접근 권한을 가집니다. 플러그인 페이지의 **정보 → 실행 방식**에 둘 중 무엇인지 표시됩니다.

## SDK

Rust SDK는 [마켓플레이스 저장소](https://github.com/empty-user77/Agentty-Marketplace/tree/main/sdk/rust)의 `sdk/rust`에 있고, 그것으로 작성한 플러그인들이 옆에 함께 있습니다.

```toml
# Cargo.toml
[lib]
crate-type = ["cdylib"]

[dependencies]
agentty-plugin = { path = "../../sdk/rust" }   # 또는 해당 저장소의 git 의존성

[profile.release]
opt-level = "z"
lto = true
panic = "abort"
strip = true
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

## `Host`가 제공하는 것

| | |
|---|---|
| `set_panel(tree)` · `show_panel()` | `ui::`로 만든 패널 |
| `notify_user(kind, message)` · `set_badge(text)` | 알림과, 아이콘 위 최대 8자 |
| `copy(text)` | 클립보드에 복사 |
| `log(line)` | 플러그인 페이지의 로그 |
| `open_url(url)` | 이용자 브라우저에서 페이지 열기 |
| `fetch(request)` | HTTP 요청 — `net.request` 필요 |
| `prompt(text, target)` | 에이전트에 보낼 프롬프트 — `prompt.inject` 필요 |
| `call(method, params)` · `notify(method, params)` | 프로토콜의 나머지 전부 |
| `context()` · `now_ms()` | 이용자의 현재 위치와 시계 |

`call`과 `fetch`는 요청 id를 돌려주고, 응답은 `Plugin::answer`로 도착합니다.

모듈에는 자기 파일이 없으므로, 실행 사이에 무언가를 기억하는 수단은 `storage/get`·`storage/set`·`storage/keys`입니다. 자기 폴더의 JSON 문서 하나이고 키 64개, 총 1MB까지이며 권한이 필요 없습니다.

## 빌드와 설치

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/hello.wasm hello.wasm
```

모듈을 `agentty-plugin.json` 옆에 두고 **플러그인 → 폴더에서 설치…** 로 그 폴더를 고릅니다. 새로 빌드한 뒤에는 플러그인 페이지의 **재시작**으로 반영합니다.

## 로고

모듈에는 그림을 담을 파일 폴더가 없으므로, 모듈이 직접 그림을 들고 다닙니다. WebAssembly 커스텀 섹션 `agentty.logo`입니다. 엔진은 이 섹션을 무시하고, 마켓플레이스 엔트리의 체크섬이 이미 이 영역을 덮습니다 — 로고도 모듈의 나머지와 똑같이 리뷰된 바이트입니다. Rust에서는 static 하나입니다:

```rust
#[used]
#[link_section = "agentty.logo"]
static LOGO: [u8; 1234] = *include_bytes!("logo.png");
```

`#[used]`는 선택이 아닙니다. 아무것도 참조하지 않는 static은 릴리스 빌드에서 제거되고, 섹션도 함께 사라집니다. 배열 길이는 파일 크기와 같아야 합니다.

바이트는 512KB 이하의 PNG, JPEG, GIF, WebP여야 하며 설치할 때 확인합니다. SVG는 어떤 이름을 달고 있든 거부됩니다 — [로고](/docs/plugin-manifest#로고) 참고. Agentty는 설치 시점에 그림을 파일로 써 두므로, 목록을 그릴 때마다 모듈을 파싱하지 않습니다.

## 제한

| | |
|---|---|
| 모듈 크기 | 64MB (마켓플레이스 등록은 8MB) |
| 메모리 | 64MB |
| 메시지 하나 | 16MB |
| 메시지당 작업량 | 예산이 정해져 있어, 돌아오지 않는 플러그인은 "제시간에 끝내지 못함"으로 중단 |
| 메시지 하나를 처리하는 동안 보낸 메시지 | `send`와 `log`를 합쳐 256개를 넘으면 중단 |

한 번에 메시지 하나만 처리합니다. 모듈은 메시지를 처리하는 **동안에만** 실행되므로 백그라운드 루프가 없습니다. `host/timer`는 시간이 지나면 응답되는 요청이고, 플러그인이 얻는 것은 그 응답이 전부입니다.

## 예제

둘 다 마켓플레이스 저장소에 소스까지 함께 있고, **플러그인 → 마켓플레이스**에서 설치할 수 있습니다.

- [`hello-rust`](https://github.com/empty-user77/Agentty-Marketplace/tree/main/src/hello-rust) — 패널과 카운터. 권한을 하나도 요구하지 않습니다.
- [`agent-rest-client`](https://github.com/empty-user77/Agentty-Marketplace/tree/main/src/agent-rest-client) — 환경 변수, 저장된 요청, 프록시를 갖춘 HTTP 클라이언트. `net.request`를 씁니다.

## 다음으로

- [AgentOS 플러그인](/docs/plugin-agentos) — 에이전트를 통해 일을 진행하는 플러그인
- [매니페스트 레퍼런스](/docs/plugin-manifest) · [플러그인 프로토콜](/docs/plugin-protocol) — 모듈 ABI 포함
- [권한](/docs/plugin-permissions) · [배포하기](/docs/plugin-publishing)
