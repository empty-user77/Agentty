---
title: 플랫폼
description: macOS, Windows, Linux에서 다른 점.
---

Agentty는 macOS에서 개발되며 macOS가 기준 플랫폼입니다. 세 플랫폼 모두 다운로드가 공개되어 있고, Windows와 Linux는 AppKit이나 WebKit에 의존하지 않는 모든 기능을 동일하게 제공합니다.

## 다른 점

| | macOS | Windows | Linux |
|---|---|---|---|
| 페인 셸 | `$SHELL` | PowerShell(설치돼 있으면 `pwsh`) | `$SHELL` |
| 자격 증명 저장소 | 키체인 | 자격 증명 관리자 | Secret Service, 없으면 전용 파일 |
| 알림 | 시스템 알림, 클릭하면 페인이 열림 | 토스트 | `notify-send` |
| 인앱 브라우저 | Agentty 안에서 | 기본 브라우저 | 기본 브라우저 |
| 메뉴 바 아이콘, 미니 모드 | ✓ | — | — |
| 제목 표시줄 | Agentty 자체 | 시스템 | 시스템, 컴포지터에 장식이 없으면 Agentty 자체 |
| 설치 | DMG | 설치 프로그램(사용자 단위) | `.deb` / `.rpm` |
| 자동 업데이트 | 설치 후 재실행 | 설치 후 재실행 | 새 패키지 페이지로 연결 |

단축키는 변환됩니다. ⌘ → Ctrl+Shift, ⇧⌘ → Ctrl+Alt+Shift, ⌥⌘ → Ctrl+Alt, ⌘1…9 → Alt+1…9. [키보드 단축키](/docs/keyboard-shortcuts)를 참고하세요.

## Windows

Windows 10 버전 1809 이상, x64가 필요합니다. 설치 프로그램은 관리자 권한 없이 현재 사용자로 설치하며, 시작 메뉴 항목, `agentty://` 링크, 폴더의 "Agentty에서 열기", 그리고 `PATH`의 `agentty`를 추가합니다.

Claude Code는 훅을 Git Bash로 실행하므로 페인 상태 표시에 **Git for Windows**가 필요합니다. **설정 → 시스템 점검**이 빠진 것을 나열하고 새 탭에서 설치해 줍니다.

## Linux

X11과 Wayland를 모두 지원합니다. 패키지는 `/usr/bin/agentty`, 데스크톱 항목(응용 프로그램 메뉴와 `agentty://` 링크), 아이콘을 설치하며 GPUI가 필요로 하는 라이브러리에 의존합니다. Vulkan 드라이버 설치를 권장합니다.

업데이트는 앱에서 알려주고, 설치는 패키지 관리자가 합니다.

## 하나의 인스턴스

두 번째 실행, `agentty://` 링크, "Agentty에서 열기"는 실행 중인 Agentty에 작업을 넘기고 종료합니다. 그래서 두 프로세스가 설정·워크스페이스 파일을 공유하는 일이 없습니다.