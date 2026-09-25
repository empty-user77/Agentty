---
title: 설정 레퍼런스
description: Agentty가 보관하는 모든 파일과 settings.json의 모든 키.
---

Agentty는 모든 것을 `~/.agentty/`에 저장합니다. 대부분은 **설정**(⌘,)에서 바꾸는 편이 쉽습니다.

## 파일

| 경로 | 용도 |
|---|---|
| `~/.agentty/settings.json` | 환경설정 |
| `~/.agentty/workspaces.json` | 워크스페이스, 탭, 분할, 그룹 |
| `~/.agentty/themes/*.itermcolors` | 가져온 색 테마 |
| `~/.agentty/pricing.json` | Claude 외 모델의 가격 |
| `~/.agentty/handoffs/` | Session Flow와 에이전트 이전이 만든 컨텍스트 문서 |
| `~/.agentty/connectors.json` | API 커넥터 정의(비밀 값 없음) |
| `~/.agentty/agent-auth.json` | 새 에이전트 탭의 로그인 방식(비밀 값 없음) |
| `~/.agentty/codex-home/` | API 키 로그인을 위한 전용 Codex 홈 |
| `~/.agentty/worktrees/` | 세션별로 만들어진 워크트리 |
| `~/.agentty/plugins/` | 설치된 플러그인 |
| `~/.agentty/plugin-data/<id>/` | 각 플러그인의 데이터 |
| `~/.agentty/prompts/` | 파일로 전달되는 긴 프롬프트 |
| `~/.agentty/install_id` | 사용 통계용 무작위 설치 식별자 |
| `agentty.json`(프로젝트) 또는 `~/.agentty/commands.json` | 명령 팔레트의 사용자 항목 |

비밀 값은 이 파일들에 들어가지 않습니다. 운영체제의 자격 증명 저장소에 있습니다.

## settings.json

```json
{
  "language": "ko",
  "theme": "Agentty Dark",
  "fontFamily": "JetBrains Mono",
  "fontSize": 13.0,
  "lineHeight": 1.25,
  "cursorShape": "block",
  "scrollback": 10000,
  "askDirectory": true,
  "autoWorktree": true,
  "agentTasks": true,
  "analytics": true,
  "agentBarPosition": "top"
}
```

| 키 | 값 |
|---|---|
| `language` | `en`, `ko`, `ja`, `zh` |
| `theme` | 기본 테마 이름, 또는 가져온 `.itermcolors`의 확장자 없는 파일 이름 |
| `fontFamily`, `fontSize`, `lineHeight`, `padding` | 모양 |
| `cursorShape` | `block`, `beam`, `underline`. `cursorBlink`는 깜박임 |
| `letterSpacing` | 터미널 셀마다 더해지는 너비(pt, 0–8) |
| `boldText` | 일반 텍스트를 굵게 그림. 원래 굵던 글씨는 한 단계 더 굵어짐 |
| `colorBackground`, `colorForeground`, `colorCursor`, `colorSelection` | 직접 바꾼 테마 색. `0xRRGGBB` 값을 숫자로 적음 — JSON에는 16진 리터럴이 없어 `0x121212`는 `1184274` |
| `scrollback` | 터미널마다 유지할 줄 수 |
| `askDirectory` | 새 워크스페이스의 폴더를 묻기. `askDirectoryForTabs`는 탭에 대해 동일 |
| `resumeBar` | 예전 세션이 있는 폴더에 들어가면 이어가기를 제안 |
| `autoWorktree` | 작업 중인 프로젝트의 두 번째 세션에 전용 워크트리 제공 |
| `agentTasks` | 에이전트가 병렬 작업 시작을 요청할 수 있게 하되 매번 확인 |
| `agentGuide` | 여기서 시작한 에이전트에게 Agentty 명령 안내를 전달 |
| `stopServersOnClose` | 페인을 닫으면 거기서 시작한 서버를 종료 |
| `confirmClose` | 사용한 것을 닫기 전에 확인 |
| `agentBarPosition` | `top` 또는 `bottom` |
| `workspaceSearchBar` | 작업공간 목록 위 검색창. 이름과 그 안에서 나눈 대화를 검색(기본 켬) |
| `sortFinishedToTop` | 에이전트가 끝낸 작업공간을 맨 위로 올림(기본 끔. 끄면 직접 정리한 순서 유지) |
| `hud` | 상태 바 항목 순서: `model`, `context`, `usage`, `status`, `elapsed`, `links`, `spacer`, `ports`, `worktree`, `branch`, `folder` |
| `browser.autoOpenServers` | 개발 서버가 응답하면 인앱 브라우저에서 열기 |
| `linkOpener` | ⌘-클릭이 인앱 브라우저를 쓸지 기본 브라우저를 쓸지 |
| `externalEditor` | `auto`, `vsCode`, `cursor`, `system` |
| `harnessDetect`, `harnessPatterns`, `harnessSubmit`, `harnessAgent` | 하네스 탐지와 동작 |
| `systemNotifications`, `notifyWhenFocused`, `notifyAnswerRequests`, `chatNotify` | 알림 |
| `chatNotify.slackBot`, `chatNotify.discordBot` | 웹훅 URL 대신 봇 토큰과 채널로 보내기 |
| `chatNotify.slackChannel`, `chatNotify.discordChannel` | 봇이 쓸 채널. Slack은 `#general`·`general`·채널 ID, Discord는 채널 ID |
| `analytics` | 익명 사용 통계 동의. `false`이거나 `DO_NOT_TRACK=1`이면 아무것도 보내지 않음 |
| `menuBar` | 메뉴 바 아이콘(macOS) |

## 사용자 명령

프로젝트의 `agentty.json`이나 모든 프로젝트에 적용되는 `~/.agentty/commands.json`으로 명령 팔레트에 항목을 추가합니다.

```json
{
  "commands": [
    { "label": "Run tests", "command": "npm test" },
    { "label": "Deploy staging", "command": "./scripts/deploy.sh staging" }
  ]
}
```

## pricing.json

가격이 내장되지 않은 모델을 위한 100만 토큰당 USD 값입니다.

```json
{
  "some-model": { "input": 1.25, "output": 10.0, "cacheRead": 0.125 }
}
```

`cacheRead`, `cacheWrite`, `cacheWrite1h`는 선택이며 각각 `input`의 10%, 125%, 200%가 기본값입니다.
