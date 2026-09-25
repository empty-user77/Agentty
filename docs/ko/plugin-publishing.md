---
title: 배포하기
description: 마켓플레이스에 플러그인을 등록하거나, Git 저장소나 폴더로 직접 배포하기.
---

플러그인을 사람들에게 전달하는 방법은 둘이고, 어느 쪽을 쓸 수 있는지는 플러그인의 종류에 달려 있습니다.

| | |
|---|---|
| **마켓플레이스** | 소스가 공개된 WebAssembly 모듈. **플러그인 → 마켓플레이스**에 표시되고, 한 번에 설치되며, 체크섬으로 검증됩니다. |
| **직접 배포** | 프로그램으로 실행되는 것까지 포함한 모든 플러그인. 이용자가 직접 고른 Git 저장소나 폴더에서 설치합니다. |

## 마켓플레이스

[Agentty-Marketplace](https://github.com/empty-user77/Agentty-Marketplace)가 Agentty가 읽는 목록입니다. 플러그인은 풀 리퀘스트로 추가됩니다. Agentty 자체 플러그인도 같은 조건으로 그곳에 있습니다. 애플리케이션에 내장된 것은 없습니다.

목록에 오르는 플러그인은 **소스가 공개된 WebAssembly 모듈**입니다. 규칙은 이것뿐이고, 양쪽 모두가 중요합니다.

- **WebAssembly**여야 합니다. Agentty가 직접 실행하므로 프로토콜이 허용한 것만 건드립니다. 프로그램으로 실행되는 플러그인은 여러분이 가진 모든 것을 가지며, Agentty는 그런 것을 인터넷의 목록에서 설치하지 않습니다.
- **소스가 공개**돼야 합니다. 목록의 모듈은 바이너리이기 때문입니다. 누구나 무엇으로 빌드했는지 읽고 다시 빌드해 볼 수 있어야 합니다.

### 설치할 때 일어나는 일

1. Agentty가 HTTPS로 `index.json`을 읽습니다. 모든 항목을 여기서 다시 검사합니다. id, 문구, 권한, 모듈이 오는 호스트까지요. 검사를 통과하지 못한 항목은 표시되는 대신 목록에서 빠집니다.
2. 플러그인 페이지가 무엇인지, 소스가 어디인지, 라이선스, 모듈 크기와 체크섬, 그리고 **무엇을 할 수 있는지**를 완전한 문장으로 권한 아래에 보여줍니다.
3. **설치**를 누르면 Agentty가 모듈을 내려받아 그 체크섬과 대조합니다. 일치하기 전에는 플러그인 폴더에 아무것도 들어가지 않습니다.

### 등록하기

1. **모듈을 빌드하고 공개합니다.** 보통은 플러그인 저장소의 GitHub 릴리스입니다.

   ```bash
   cargo build --release --target wasm32-unknown-unknown
   cp target/wasm32-unknown-unknown/release/your_plugin.wasm your-plugin.wasm
   shasum -a 256 your-plugin.wasm
   wc -c your-plugin.wasm
   ```

2. **항목을 작성합니다.** `plugins/_template.json`을 `plugins/<플러그인-id>.json`으로 복사하세요. id는 파일 이름과 `agentty-plugin.json`의 `id`와 같아야 합니다.

   ```json
   {
     "id": "hello-world",
     "name": "Hello World",
     "version": "0.1.0",
     "publisher": "Your Name",
     "description": "One sentence about what it does.",
     "icon": "sparkles",
     "license": "MIT",
     "source": "https://github.com/you/agentty-hello-world",
     "build": {
       "repository": "https://github.com/you/agentty-hello-world",
       "rev": "3f2b1c9e4a7d05b8c6e1f0a2d4b83c7e9015d6af",
       "path": ".",
       "toolchain": "1.98.1",
       "artifact": "target/wasm32-unknown-unknown/release/hello_world.wasm"
     },
     "keywords": ["example"],
     "apiVersion": 1,
     "surface": "sidebar",
     "mode": "push",
     "permissions": [],
     "module": {
       "url": "https://github.com/you/agentty-hello-world/releases/download/v0.1.0/hello-world.wasm",
       "sha256": "…64자리 16진수…",
       "size": 93292
     }
   }
   ```

3. **검사한 뒤 풀 리퀘스트를 엽니다.** CI가 같은 검사를 실행하고, 항목과 그것이 가리키는 모듈과 소스를 다시 빌드한 결과가 모두 일치하지 않으면 거부합니다.

   ```bash
   python3 scripts/validate.py              # 모든 항목의 형식·호스트·권한·빌드 블록
   python3 scripts/validate.py --download   # 각 모듈을 내려받아 체크섬까지 확인
   python3 scripts/validate.py --source     # 소스를 누구나 읽을 수 있는지까지 확인
   python3 scripts/verify_build.py          # 소스를 다시 빌드해 체크섬과 대조
   ```

`verify_build.py`에는 Docker가 필요합니다. 다이제스트로 고정한 `rust` 이미지 안에서 모듈을 빌드하므로, 빌드한 기계에 따라 바이트가 달라지지 않습니다. CI가 여러분과 같은 체크섬에 도달할 수 있는 이유입니다.

### 항목의 필드

| 필드 | |
|---|---|
| `id` | 2~40자의 `a-z 0-9 -`. 파일은 `plugins/<id>.json` |
| `name`, `version`, `description` | Agentty에 표시됨. `name`은 60자, `description`은 300자까지. `version`은 `major.minor.patch` |
| `publisher`, `license` | 만든 사람과 라이선스 |
| `source` | 모듈을 빌드한 공개 저장소 — **필수**. `github.com`, `gitlab.com`, `codeberg.org`, `git.sr.ht` 중 하나면 되고, GitHub일 필요는 없습니다. `build.repository`와 같은 저장소여야 합니다 — 항목이 가리키는 코드가 실제로 배포되는 코드여야 하기 때문입니다 |
| `build` | **필수** — 그 모듈을 다시 빌드하는 방법. `repository`(클론 주소), `rev`(40자 전체 커밋. 태그나 브랜치는 나중에 옮겨질 수 있어 안 됩니다), `path`(저장소 안 플러그인 디렉터리, 또는 `.`), `toolchain`(마켓플레이스가 쓰는 Rust 버전), `artifact`(빌드가 만드는 `.wasm`, `path` 기준 상대 경로). 어느 것도 명령이 아닙니다 — 무엇을 실행할지는 `scripts/verify_build.py`에 고정되어 있습니다 |
| `homepage`, `keywords`, `icon` | 선택. 아이콘은 Agentty 아이콘 집합의 이름 |
| `apiVersion` | 모듈이 대상으로 한 플러그인 프로토콜. `1`이면 생략 |
| `surface` | 아이콘 위치: `sidebar`, `pane`(기본), `status` |
| `mode` | 패널이 열리는 방식: `push`(기본), `overlay`, `window`, `full` |
| `permissions` | 요청하는 권한. 설치 전에 표시되고, 더 요구하는 업데이트에서 다시 표시됩니다 |
| `module.url` | `github.com`, `raw.githubusercontent.com`, `objects.githubusercontent.com` 중 하나의 `https://` 주소. 경로에 버전을 넣어 두면 릴리스를 몰래 바꿔치기할 수 없습니다 — 검사 항목이 아니라 관례입니다 |
| `module.sha256` | 체크섬. 일치하지 않는 다운로드는 거부되고, 일치하더라도 WebAssembly 모듈이 아닌 바이트는 거부됩니다 |
| `module.size` | 바이트 단위의 정확한 길이. 최대 8MB. 상한이 아니라 정확한 값입니다 — 길이가 다른 다운로드는 거부되므로, 빌드할 때마다 바뀝니다 |

등록된 플러그인은 엔트리에 로고를 담지 않습니다. 아이콘 대신 이미지를 쓰고 싶다면 모듈의 `agentty.logo` 섹션에 넣으세요. 그러면 나머지와 마찬가지로 `module.sha256`이 덮게 되고, 누군가 플러그인을 설치할 때 게시자 서버로 아무 요청도 나가지 않습니다. [로고](/docs/plugin-rust#로고) 참고.

### 거부되는 경우

- `build.repository`를 `build.rev`에서 다시 빌드해도 CI가 같은 바이트에 도달하지 못하는 모듈이거나, 아무도 읽을 수 없는 저장소인 경우.
- `build.path`의 매니페스트가 항목과 어긋나는 경우 — id·버전·`apiVersion`·권한 목록이 다른 경우.
- 체크섬이 URL이 제공하는 것과 다른 경우.
- 쓰지 않는 권한을 요청하거나, 그 권한으로 무엇을 하는지 설명에 없는 경우.
- 다른 플러그인, 다른 배포자, 또는 Agentty 자체인 척하는 경우.

### 업데이트

`version`, `module.url`, `module.sha256`, `module.size`를 바꾸고 풀 리퀘스트를 다시 엽니다. 설치한 모든 사람에게 업데이트가 제안됩니다.

설치된 버전보다 **더 많은** 권한을 요구하는 업데이트는 무엇이 추가되는지 밝히고 한 번 더 눌러야 적용됩니다. **모두 업데이트**는 그런 것들을 조용히 가져가는 대신 빼놓고, 몇 개를 남겼는지 알려 줍니다. 그러니 권한을 늘리면 다시 들여다보지 않는 이용자를 잃습니다. 설명을 그들이 들여다보게끔 쓰세요.

새 버전이 최신 Agentty에만 있는 기능을 쓴다면 `apiVersion`도 함께 올리세요. 구버전 Agentty를 쓰는 사람은 실행할 수 없는 모듈을 받는 대신 기존 버전을 유지한 채 업데이트 안내를 받습니다. Agentty는 실행할 수 없는 버전을 설치된 것의 업데이트로 치지 않습니다.

항목을 지우면 Agentty가 그 플러그인을 더 이상 제공하지 않습니다. 이미 설치한 사람에게는 남아 있고, 플러그인 페이지에서 직접 삭제할 수 있습니다.

> [!NOTE]
> `AGENTTY_MARKETPLACE_INDEX`로 다른 목록을 가리키게 해서 목록을 작성하는 동안 시험해 볼 수 있습니다. 모듈은 저장소의 릴리스나, 목록과 같은 호스트에서 제공될 수 있습니다.

## 직접 배포하기

위의 것은 플러그인을 쓰는 데 필수가 아닙니다. 혼자 쓰거나 회사 안에서만 쓰는 플러그인은 그 목록을 거칠 필요가 없습니다.

### Git 저장소로

`agentty-plugin.json`을 저장소 **루트**에 두세요. 이용자는 *직접 만들기* 아래에 `https://` 주소를 붙여넣고 **Git에서 설치**를 누릅니다.

```
your-plugin/
├── agentty-plugin.json
├── main.mjs
├── agentty-plugin.mjs     번들된 SDK
├── README.md
└── LICENSE
```

> [!IMPORTANT]
> **SDK와 `node_modules`를 포함해 실행에 필요한 모든 것을 번들하세요.** Agentty는 `npm install`을 실행하지 않고, 진입점을 있는 그대로 시작합니다. 번들되지 않은 의존성이 있는 플러그인은 이용자 컴퓨터에서 module-not-found 오류로 실패하고 로그에만 남습니다.

가능하면 의존성 없는 코드를 쓰세요. SDK 자체에 의존성이 없는 이유도 정확히 이것입니다.

### 폴더로

**폴더에서 설치…** 는 폴더를 `~/.agentty/plugins/`로 복사합니다. 올바른 이름의 폴더로 풀리는 zip도 같은 방식으로 동작합니다. 폴더 이름은 매니페스트의 `id`와 같아야 합니다.

## 버전 관리

`version`은 `major.minor.patch`입니다. 공개할 때마다 올리세요. Agentty가 이 값을 비교해 설치된 플러그인에 업데이트가 있는지 판단합니다.

매니페스트와 코드를 함께 유지하세요. 더 이상 쓰지 않는 권한은 매니페스트에서도 빼고, 이름을 바꾼 명령이 `contributes`에 남아 있지 않게 하세요.

## 다음으로

- [Rust와 WebAssembly](/docs/plugin-rust) — 모듈 빌드하기
- [매니페스트 레퍼런스](/docs/plugin-manifest) · [권한](/docs/plugin-permissions)
