---
title: 매니페스트 레퍼런스
description: agentty-plugin.json의 모든 필드 — 식별 정보, 실행 방식, 권한, 패널, 명령, 아이콘.
---

`agentty-plugin.json`은 플러그인 폴더 최상위에 놓이며, 이 플러그인이 무엇이고 어떻게 시작하며 무엇을 할 수 있고 인터페이스에 무엇을 더하는지 Agentty에 알려줍니다.

```json
{
  "id": "hello",
  "name": "Hello",
  "version": "0.1.0",
  "main": "main.mjs",
  "runtime": "node",
  "apiVersion": 1,
  "description": "Turns the current folder into a prompt.",
  "publisher": "you",
  "homepage": "https://example.com/hello",
  "permissions": ["prompt.inject"],
  "contributes": {
    "panel": { "title": "Hello", "icon": "sparkles" },
    "commands": [
      { "id": "hello.explain", "title": "Hello: Explain this folder", "icon": "bot" }
    ]
  }
}
```

## 식별 정보

| 필드 | 필수 | 설명 |
|---|---|---|
| `id` | 예 | `a-z`, `0-9`, `-`로 이루어진 2–40자. **폴더 이름과 같아야 합니다.** |
| `name` | 예 | 스토어와 패널 버튼에 표시 |
| `version` | 예 | `major.minor.patch` |
| `description` | | 스토어 카드의 한 줄 설명 |
| `publisher` | | 제작자 |
| `homepage` | | `https://` 여야 합니다 |
| `keywords` | | 스토어 검색용 |
| `links` | | 카드에 버튼으로 표시되는 `{ "label", "url" }` 최대 6개 (프로젝트 사이트, 문서, 소스) |
| `icon` | | 아래 목록의 아이콘 이름 |
| `logo` | | 플러그인 폴더 안의 그림 파일. `icon` 대신 그려집니다. 모듈은 로고를 모듈 안에 담습니다 — [로고](#로고) 참고 |

## 실행

| 필드 | 기본값 | 설명 |
|---|---|---|
| `main` | 필수 | 플러그인 폴더 기준 진입점 |
| `runtime` | `node` | `node`(로그인 셸 `PATH`의 Node.js 18+), `python`(`python3 <main>`), `executable`(`<main>`을 직접 실행), 또는 `wasm` — `<main>`이 Agentty가 직접 실행하는 WebAssembly 모듈. [Rust와 WebAssembly](/docs/plugin-rust) 참고 |
| `apiVersion` | `1` | 작성 기준이 된 플러그인 API 버전. `2`는 [AgentOS 플러그인](/docs/plugin-agentos)에 필요한 `host/timer`와 `pane/status`를 더합니다. 더 낮은 버전만 아는 Agentty는 실행할 수 없는 것을 설치하는 대신 그렇다고 알립니다 |
| `activationEvents` | `[]` | `["onStartup"]`이면 Agentty와 함께 시작하고, 아니면 처음 사용할 때 시작 |

Agentty는 플러그인 폴더를 작업 디렉터리로 삼아 프로그램을 실행합니다. `wasm` 플러그인은 프로그램을 시작하지 않습니다. 모듈이 Agentty 안에서 실행되며 작업 디렉터리도, 환경 변수도, 파일도 없습니다.

## 다른 앱과의 연동

| 필드 | 설명 |
|---|---|
| `requires` | `{ "name", "url", "note" }` — 이 플러그인이 대상으로 하는 앱이나 서비스. 카드가 발견 여부를 표시하고, 없으면 링크를 제공합니다 |
| `detect` | 연동 대상 앱의 경로(`~` 사용 가능). 발견되면 카드가 **추천**으로 표시됩니다 |

## 권한

```json
"permissions": ["net.request", "prompt.inject", "terminal.write", "session.read", "workspace.read"]
```

| 권한 | 허용되는 일 |
|---|---|
| `net.request` | 플러그인이 지정한 주소로 HTTP 요청 |
| `prompt.inject` | 프롬프트 전송 |
| `terminal.write` | 열린 페인에 입력 |
| `session.read` | AI 대화 읽기 |
| `workspace.read` | 워크스페이스 목록 조회, 컨텍스트의 폴더·제목 필드 확인 |

쓰는 것만 요청하세요. 목록은 설치 전에 이용자에게 표시되고, 권한 없는 호출은 실패합니다. [플러그인 권한](/docs/plugin-permissions)을 참고하세요.

## 플러그인이 더하는 것

### 패널

```json
"contributes": { "panel": { "title": "Hello", "icon": "sparkles", "surface": "sidebar", "mode": "push" } }
```

플러그인에 버튼과, UI 트리로 채우는 패널(너비 360px, 세로 스크롤)을 줍니다. [SDK](/docs/plugin-sdk)를 참고하세요.

`surface`는 버튼이 놓이는 자리를 정합니다.

| `surface` | 위치 |
|---|---|
| `pane`(기본) | 터미널 위 탭 영역 |
| `sidebar` | 왼쪽 가장자리의 액티비티 바, Agentty 자체 페이지들과 나란히 |
| `status` | 아래쪽 상태 바 |

`mode`는 패널이 열리는 방식을 정합니다. 이용자가 패널의 레이아웃 버튼으로 바꿀 수 있고 그 선택이 유지됩니다. 처음 동작은 이렇습니다.

| `mode` | |
|---|---|
| `push`(기본) | 터미널 옆에 도킹되고, 터미널이 자리를 비켜 줍니다 |
| `overlay` | 창 오른쪽 가장자리에 떠 있고, 다른 것은 움직이지 않습니다 |
| `window` | 독립된 창. 이동과 크기 조절이 가능합니다 |
| `full` | 터미널과 페이지가 쓰는 영역 전체 |

도킹된 패널이 나머지 창을 짓누를 만큼 커지지는 않습니다. 도킹 가능한 범위를 넘겨 끌면 떠 있는 패널이 됩니다.

### 명령

```json
"contributes": {
  "commands": [
    {
      "id": "hello.explain",
      "title": "Hello: Explain this folder",
      "description": "Sends a tour request to the focused agent",
      "icon": "bot",
      "palette": true
    }
  ]
}
```

| 필드 | 설명 |
|---|---|
| `id` | 플러그인 안에서 고유. SDK가 이 id로 핸들러를 등록합니다 |
| `title` | 팔레트에 표시. 플러그인 이름을 앞에 붙이면 묶여 보입니다 |
| `description` | 선택적 두 번째 줄 |
| `icon` | 팔레트에서 명령 옆에 표시되는 아이콘 이름 |
| `palette` | `false`면 명령 팔레트에서 숨김 |

명령은 명령 팔레트에서 실행합니다. `paneBar`와 `when`은 없어졌습니다. 두 필드가 남아 있는 매니페스트도 그대로 설치·실행되고 필드는 무시되지만, **그 버튼에 의존해 만든 플러그인은 버튼을 잃고** 명령은 팔레트에서 실행하게 됩니다.

## 아이콘

`icon` 필드에는 아래 이름을 씁니다. 그 외의 값은 퍼즐 조각으로 표시됩니다.

```
app-window arrow-down arrow-left arrow-right arrow-up arrow-up-right at-sign bell bell-dot
blocks book-open bookmark bot brain bug calendar chart-column check chevron-down chevron-right
chevron-up circle-check circle-dot circle-pause circle-x clipboard clipboard-paste clock cloud
code columns-2 command container copy database download ellipsis external-link eye file-input
file-plus file-text folder folder-open folder-plus git-branch git-commit-horizontal
git-pull-request globe grip-vertical hammer hash history house image info key-round
layout-panel-left lightbulb link list list-tree loader-circle mail maximize-2 message-circle-question
message-square minimize-2 minus network notebook notebook-pen package panel-left-close
panel-left-open pencil picture-in-picture-2 play plug plus power puzzle refresh-cw rocket rotate-cw
rows-2 save scroll-text search send settings shield-alert sparkles square square-plus
square-terminal star sticky-note tag terminal trash-2 undo-2 unlink upload users wand-sparkles
workflow wrench x zap git-fork file lock graduation-cap x-twitter
```

## 로고

로고는 플러그인 고유의 이미지입니다. 스토어 목록, 상세 카드, 패널 버튼에서 `icon` 이름 대신 이 이미지가 그려집니다.

**로고는 어디에서도 받아오지 않습니다.** 플러그인과 함께 따라오므로, 화면에 그려지는 그림은 사용자가 설치한 바로 그 그림입니다. 그리고 로고를 넣는다고 해서 플러그인 작성자가 누가 설치했는지 알게 되지 않습니다.

| 플러그인 종류 | 로고가 있는 곳 |
|---|---|
| 폴더 — `node`, `python`, `executable`, 또는 개발용으로 연결한 폴더 | `"logo": "logo.png"`, 플러그인 폴더 안의 파일 |
| 단일 `wasm` 모듈 — 스토어에서 오는 모든 것 | 모듈이 그림을 직접 들고 있습니다. `logo`는 쓰지 않습니다. [Rust와 WebAssembly](/docs/plugin-rust#로고) 참고 |

매니페스트 필드로서의 `logo`는 파일 이름일 뿐입니다. 플러그인 폴더 밖으로 나가는 경로(`../`), 절대 경로, 스킴이 붙은 값(`https://` 주소 포함), 400자를 넘는 값은 모두 거부되고 플러그인은 `icon`을 그대로 씁니다.

모듈이 들고 있는 그림은 **PNG, JPEG, GIF, WebP 중 하나**여야 하고, 이름이 아니라 **바이트로 실제 형식을 확인**하며, 512KB 이하여야 합니다. 그 밖의 것은 로고가 아니고, 플러그인은 `icon`을 그대로 씁니다.

**모듈의 로고는 SVG일 수 없습니다.** 어떤 이름을 달고 있든 마찬가지입니다. SVG는 그림이 아니라 문서이기 때문입니다 — 렌더러가 그 안에 적힌 주소를 열기 때문에, 로컬 경로가 적혀 있으면 그 파일이 열려 그려집니다. 플러그인이 로고라는 이름으로 사용자의 파일을 화면에 띄울 수 있어서는 안 됩니다.

## 환경 변수

프로그램으로 실행되는 플러그인(`node`, `python`, `executable`)은 다음 환경 변수를 받습니다. `wasm` 플러그인은 아무것도 받지 않고, 필요한 것을 [자신의 저장소](/docs/plugin-permissions)에 둡니다.

| 변수 | 의미 |
|---|---|
| `AGENTTY_PLUGIN_ID` | 플러그인 id |
| `AGENTTY_PLUGIN_DIR` | 플러그인 폴더 |
| `AGENTTY_PLUGIN_DATA` | 설정과 캐시를 위한 전용 폴더 |
| `AGENTTY_VERSION` | Agentty 버전 |
| `AGENTTY_LANGUAGE` | 이용자 언어 (`en`, `ko`, `ja`, `zh`) |
| `AGENTTY_BIN` | `agentty` 명령줄 도구 경로 |

저장할 것은 모두 `AGENTTY_PLUGIN_DATA`에 두거나, 두 종류 모두에서 동작하는 `storage/*`를 쓰세요.
