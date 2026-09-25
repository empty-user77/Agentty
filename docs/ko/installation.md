---
title: 설치
description: macOS·Windows·Linux용 Agentty를 내려받고, 함께 쓸 에이전트 CLI를 준비하는 방법.
---

Agentty는 무료이고 계정이 필요 없습니다. 내려받아 드래그해 넣고 터미널을 열면 됩니다.

## 내려받기

모든 릴리스는 [릴리스 페이지](https://github.com/empty-user77/Agentty/releases)에 공개됩니다.

**macOS 13 이상, Apple 실리콘** — `Agentty-X.Y.Z-…-arm64.dmg`를 열고 **Agentty**를 **응용 프로그램**으로 드래그하세요. Developer ID로 서명되고 Apple 공증을 받았기 때문에 경고 없이 실행됩니다.

> [!NOTE]
> Intel Mac용 빌드는 없습니다. macOS용 Agentty는 Apple 실리콘 전용입니다.

**Windows 10 1809 이상, x64** — `Agentty-X.Y.Z-windows-x64-setup.exe`를 실행하세요. 사용자 단위로 설치되며 관리자 권한이 필요 없습니다.

> [!IMPORTANT]
> **Windows 설치 프로그램은 아직 코드 서명되어 있지 않습니다.** 그래서 Windows SmartScreen이 게시자를 알 수 없다고 경고할 수 있습니다. 계속하려면 **추가 정보 → 실행**을 선택하세요. 브라우저가 `.exe` 다운로드 자체를 막는다면(Chrome은 서명 없는 실행 파일을 차단합니다) `Agentty-X.Y.Z-windows-x64-setup.zip`을 받아 압축을 푸세요. 같은 설치 프로그램이 들어 있습니다.

**Linux, x86_64** — Debian 12+ / Ubuntu 22.04+: `sudo apt install ./Agentty-X.Y.Z-linux-amd64.deb`. RHEL 9+ / Fedora: `sudo dnf install ./Agentty-X.Y.Z-linux-x86_64.rpm`.

모든 파일의 체크섬은 `Agentty-X.Y.Z-SHA256SUMS.txt`에 있습니다. 세 플랫폼의 차이는 [플랫폼](/docs/platforms)을 참고하세요.

## 에이전트 명령줄 도구

Agentty는 에이전트를 실행할 뿐 함께 배포하지는 않습니다. 원하는 도구를 설치하고 로그인 셸의 `PATH`에서 찾을 수 있게 해 두세요.

- **Claude Code** — `claude`
- **Codex** — `codex`
- 선택: Gemini CLI, Copilot CLI, Cursor CLI, Grok, OpenCode, Qwen Code, Amp, Droid, Goose, Crush, Aider, 그리고 로컬 Ollama 모델

Agentty는 설치된 것만 감지해 제안합니다. 없는 도구는 나중에 실패하는 대신 아예 표시되지 않습니다.

> [!TIP]
> 터미널에서는 되는데 Agentty에서만 안 된다면, 대개 `PATH`를 대화형 셸에서만 읽는 파일에 설정해 둔 경우입니다. Agentty는 로그인 셸로 페인을 시작하므로 로그인 셸이 읽는 위치에 `PATH`를 넣어 주세요.

CLI 자체의 로그인 없이 **설정 → 계정**에서 API 키, 게이트웨이 토큰, `claude setup-token`, Amazon Bedrock, Google Vertex AI, 또는 가져온 Codex `auth.json`으로 시작할 수도 있습니다.

### Node.js

JavaScript로 작성된 플러그인에는 `PATH` 위의 **Node.js 18 이상**이 필요합니다. Agentty 자체는 필요로 하지 않습니다.

### Git

git 페이지, 워크트리, 브랜치 전환은 `PATH`의 `git`을 사용합니다. Windows에서는 Claude Code의 훅에 **Git for Windows**가 필요합니다.

## 업데이트

Agentty는 실행 시와 매시간 새 버전을 확인합니다.

알림의 **나중에**는 다음 확인 때까지가 아니라 하루를 미룹니다. 그렇게 미뤄 둔 업데이트는 상태 바에 계속 표시되므로, 미루는 것과 잊는 것이 같아지지 않습니다.

- **macOS와 Windows** — 업데이트를 내려받아 릴리스 체크섬으로 검증한 뒤 설치하고 다시 실행합니다.
- **Linux** — 새 버전이 나왔다고 알리고 릴리스 페이지를 안내합니다. 새 `.deb` 또는 `.rpm`은 패키지 관리자로 직접 설치하세요.

## 소스에서 빌드하기

소스 저장소는 아직 공개되지 않았습니다. 공개가 예정되어 있으며, 그때까지는 릴리스 페이지의 빌드가 Agentty를 실행하는 방법입니다.

## Agentty가 파일을 두는 곳

| 경로 | 용도 |
|---|---|
| `~/.agentty/settings.json` | 환경설정 |
| `~/.agentty/workspaces.json` | 워크스페이스, 탭, 분할, 그룹 |
| `~/.agentty/plugins/` | 설치된 플러그인 |
| `~/.agentty/handoffs/` | Session Flow와 대화 이전이 만든 컨텍스트 문서 |
| `~/.agentty/connectors.json` | API 커넥터 정의 (비밀 값은 OS 자격 증명 저장소에 보관) |

전체 목록은 [설정 레퍼런스](/docs/configuration-reference)에 있습니다.

## 삭제하기

응용 프로그램 폴더에서 `Agentty.app`을 지우면 됩니다.

데이터까지 지우려면 `~/.agentty`를 삭제하세요. 에이전트 CLI가 기록한 세션은 각자의 폴더(`~/.claude`, `~/.codex`)에 있으며 Agentty가 지울 대상이 아닙니다.

## 다음으로

- [빠른 시작](/docs/quick-start) — 처음 10분
- [플랫폼](/docs/platforms) — Windows와 Linux에서 다른 점