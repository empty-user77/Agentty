# 2026-09-18 보안 · 로직 감사와 커밋/푸시 전 자동 검사

상태: ⬜ 대기 · 🔧 진행 중 · ✅ 완료 · ⚠️ 부분 완료/제약

## 요청 요약

1. 로직 · 동작 흐름 2. GitHub / Vercel / Supabase / 코드 보안 3. BOLA(외부 → 내부 권한) 4. 키 · 토큰 · 개인정보 · 로컬 파일 · env 유출 경로 5. 그 외 보안 감사 6. 개발 / 프로덕션 경로 · 설정 분리와 주입
— 핵심을 스킬로 만들어 커밋 · 푸시 전에 자동 실행 (기존 스크립트 보강, 전부 영어)

## 방법

관점별 검토 에이전트 4개(sonnet, 읽기 전용) 병렬 실행 + 6번 항목과 상위 결과는 직접 확인. "확인"은 코드나 실행으로 재현한 것, "추정"은 외부 서비스 동작에 대한 가정.

## 결과 — 수정함

| # | 심각도 | 내용 | 조치 |
|---|---|---|---|
| 1 | 높음 · 확인 | **복제한 저장소가 auto 모드를 스스로 켤 수 있었음** — `is_idea_project`가 `docs/idea/BUILD_GUIDE.md` 존재만 확인 | Agentty가 만든 프로젝트만 `data_dir()/idea-projects.json`(0600, 정규화 경로)에 기록하고 그 목록으로 판별. 가이드 파일만 넣은 폴더는 해당 없음(테스트) |
| 2 | 높음 · 코드 확인 / Vercel 동작 추정 | **프레임워크 없는 사이트를 배포하면 `docs/idea/**`(기획 메모 · 첨부) · `.claude/**`가 공개 URL로 제공될 수 있음** — 제외 장치가 전혀 없었음 | 저장 · 배포 전에 `.vercelignore` 작성(`docs/idea`, `.claude`, `supabase`, `PLAN.md`, `CLAUDE.md`, `AGENTS.md`, `.env*`…). 공개 저장소를 고르면 `docs/idea/`를 `.gitignore`에 추가하고 패널에 안내 |
| 3 | 높음 · 확인 | 기존 `origin`을 무조건 신뢰 — 변조된 폴더의 원격으로 "사이트 업데이트" 때 전체 코드가 푸시될 수 있음 | Launch가 저장한 적 없는 원격은 `host/owner/repo`를 보여 주고 한 번 확인받은 뒤에만 푸시(`confirmedOrigin`) |
| 4 | 높음 · 확인 | 대부분의 단계에 재진입 가드 없음(SDK는 이벤트를 동시 처리) — 더블클릭으로 배포 · 저장소 생성 · 도구 설치가 두 번 실행 | `runStep` 자체에 가드. 더블클릭 시 배포 1회 테스트 추가 |
| 5 | 중간 · 확인 | 아이디어 프로젝트 허용 목록의 `cat` · `cp` · `mv`가 경로 제한 없이 조용히 실행 → 주입된 지시가 `~/.ssh` 등을 읽어 내보낼 수 있는 경로 (auto 모드가 아닐 때) | 세 규칙 제거(프로젝트 안 파일은 Read / Write / Edit 사용) |
| 6 | 중간 · 확인 | `.envrc`, `*.pem`, `*.p12`, `id_rsa`, `.aws/` 등은 gitignore · 저장 거부 대상이 아니었음 | gitignore 목록 확장 + 추적 중인 키 파일이 있으면 저장 거부 |
| 7 | 중간 · 확인 | `gh` 바이너리를 무결성 확인 없이 설치 | 릴리스의 checksums 파일과 SHA-256 대조, 불일치 · 부재 시 중단 |
| 8 | 중간 · 확인 | `launch.open` 명령의 `args.path`는 검증 없이 사용(링크 경로는 검증) | 같은 검증(절대 경로 · 존재하는 폴더) 적용 |
| 9 | 중간 · 확인 | 마이그레이션 내용을 보여 주지 않고 적용 | 적용 전 파일 이름 목록 표시 |
| 10 | 중간 · 확인(재현) | `-`로 시작하는 프롬프트는 Claude Code에 옵션으로 해석됨(`unknown option`) | 프롬프트 앞에 `--` |
| 11 | 중간 · 확인 | 시작 화면의 "설치 안 됨" 카드에 클릭 리스너가 두 개 — 설치 안내와 함께 빈 셸 워크스페이스가 열림(GPUI는 리스너를 누적 실행) | 카드 모양과 실행 리스너를 분리 |
| 12 | 중간 · 확인 | 앱 시작 시 복원되는 창이 에이전트 감지보다 먼저 떠서 auto 모드 플래그가 빠짐 | 마지막 감지 결과를 데이터 폴더에 보관해 첫 감지 전까지 사용 |
| 13 | 중간 · 확인 | **키체인 서비스명이 개발 / 프로덕션 공통**(`run.agentty.connector`), 커넥터 id는 이름 슬러그 → 개발 빌드에서 같은 이름을 추가 · 삭제하면 프로덕션 비밀을 덮어쓰거나 지움 | `AGENTTY_DATA_DIR`가 있으면 서비스명에 데이터 폴더 해시를 붙임 |
| 14 | 중간 · 확인 | 시크릿 스캐너 빈틈: 따옴표 없는 `KEY=value`, 이름이 앞에 오는 Apple 앱 암호, Supabase / GitLab / HF / DB URL 등 패턴 부재, 바이너리 · 파일명 미검사 | 패턴 24종 추가, 파일명 검사는 `security-audit.py` SA01 |
| 15 | — · 확인 | **이 클론에서 git 훅이 꺼져 있었음**(`core.hooksPath` 미설정) — 커밋 · 푸시 때 아무 검사도 돌지 않았음 | `scripts/install-hooks.sh` 실행 |

## 결과 — 열려 있음 (결정 필요)

| # | 심각도 | 내용 | 권고 |
|---|---|---|---|
| A | 중간 · 확인 | **시그널 소켓(`$TMPDIR/agentty-<pid>.sock`, 0600)에 연결 인증과 발신자–창 결속이 없음.** 같은 사용자 계정의 어떤 프로세스든(권한을 하나도 선언하지 않은 플러그인 포함) `browser` 명령으로 내장 브라우저에서 JS 실행 · 페이지 읽기, 다른 창의 상태 · 알림 위조 가능. 다른 macOS 계정에서는 불가. 플러그인은 애초에 사용자 권한의 Node 프로세스(문서화된 신뢰 모델)라 경계를 새로 뚫는 것은 아니지만 매니페스트 권한 표시와 어긋남 | 앱 시작 시 난수 토큰 발급 → Agentty가 띄운 창 · MCP에만 환경변수로 전달, 소켓 첫 줄에서 검증. 브라우저 제어는 창별 토큰에 결속 |
| B | 중간 · 확인 | 플러그인의 `session.read` / `terminal.write` 범위가 "플러그인이 호출된 워크스페이스"가 아니라 창 전체 | `paneId` 조회를 호출 컨텍스트의 워크스페이스로 제한 |
| C | 중간 · 설계 | ECC는 기본 브랜치 최신을 받음 — 외부 저장소 내용이 auto 모드 에이전트의 지침이 됨. Markdown만 설치 · 스크립트 금지 규칙은 있으나 내용 자체의 주입은 막지 못함 | 릴리스마다 검토한 커밋으로 고정 |
| D | 낮음 · 확인 | `build-dmg.sh`가 `notarytool`에 Apple ID · 앱 암호를 인자로 전달(`ps`에 노출), `projects create --db-password`도 동일 | `notarytool store-credentials` + `--keychain-profile` |
| E | 중간 · 추적 | `redact_args` / `redact_url`이 `-H "Cookie: session=…"` 같은 결합 문자열은 가리지 못함(확장 화면의 MCP 설정 표시) | 값 안의 `name=value` · `Name: value` 형태도 마스킹, 테스트 추가 |
| F | 낮음 | `create_project`의 이름 선점 경합, 에이전트 실행 실패 시 프로젝트 폴더가 남음 · npm `@latest` 미고정 · PATH에 먼저 있는 `gh` / `vercel` 우선 | 필요 시 개별 처리 |

## 확인했고 문제없던 것

- `agentty://` 링크: 프롬프트는 항상 확인 대화상자 · 자동 전송 없음, 링크로 플러그인 자동 설치 불가, 링크를 받은 플러그인은 이후 터미널 전송 금지(link guard)
- 플러그인 RPC 권한은 호스트에서 메서드마다 검사 · `host/openUrl`은 http(s)만 · 플러그인 설치는 폴더 복사(심볼릭 링크 미추적) / https git만
- 내장 브라우저: 페이지 JS → 앱 브리지 없음 · 브라우저 MCP는 stdio(포트 없음) · 디버그 드라이버는 앱 자신의 `AGENTTY_DEBUG=1`에서만
- 에이전트에 주입하는 훅 명령은 고정 문자열(저장소 내용이 끼어들 곳 없음) · `shell_quote`는 따옴표 · 백틱 · `$()` · 개행 안전, `-c` 실행에서는 `!` 히스토리 확장 없음(zsh / bash 실행 확인)
- 분석: 이벤트 · 속성 허용 목록(경로 · 프롬프트 · 제목 없음), 자격은 빌드 시점에만 주입 · 업데이트는 github.com 한정 + SHA-256 · 프롬프트 / 핸드오프 / 커넥터 파일 0600, 폴더 0700
- 개발 / 프로덕션: 모든 상태가 `fsutil::data_dir()` 하나로, 시작 시 상속된 `AGENTTY_SOCKET` · `PANE_ID` 등 제거, 소켓은 pid별, 업데이트 설치는 번들 앱에서만, CI는 `permissions: contents: read` · SHA 고정 액션 · `pull_request_target` 없음

## 자동 검사 (보강)

| 층 | 내용 |
|---|---|
| `scripts/check-secrets.py` | 패턴 24종 추가(Supabase · GitLab · Hugging Face · SendGrid · Groq · xAI · Replicate · DigitalOcean · Shopify · Docker Hub · Atlassian · PyPI · Google OAuth · Twilio · Mailgun · Telegram · DB URL · 비밀번호가 든 URL · 따옴표 없는 env 대입 · Apple 앱 암호) |
| `scripts/security-audit.py` (신규) | SA01 커밋 금지 파일(이름 기준: env · 키 · 전사 · 로컬 도구 상태 · 아카이브) · SA02 실제 홈 경로(차단) / 이메일(경고) · SA03 보장을 약화하는 코드(권한 우회 · 넓은 허용 규칙 · `--reveal` · 훅 건너뛰기 · 자격 스크립트의 `set -x` · `pull_request_target` · 비영어 프롬프트) · SA04 가드 배선 확인. 주석 · 테스트 · 바이너리는 제외, 오탐은 `audit: ok — 이유` |
| `.claude/skills/security-audit` (신규) | 판단이 필요한 6개 영역 체크리스트 + 민감 코드 위치 표 + 보고 형식. 마지막 단계에서 검토한 트리를 기록(`--mark`) |
| `.claude/hooks/require-security-audit.py` (신규) | 스테이징된 트리가 검토 기록과 일치하지 않으면 `git commit` / `git push` 거부. `git commit -a`, 스테이징과 커밋을 한 명령에 묶는 것도 거부 |
| git 훅 · CI | pre-commit / pre-push / CI에 `security-audit.py` 추가 |

## 검증

| # | 항목 | 결과 |
|---|---|---|
| 1 | fmt · clippy · `cargo test --workspace` | ✅ app 90 / bridge 79 |
| 2 | `node --test plugins/launch/test/plugin.test.mjs` | ✅ 33개 (체크섬 · 키 파일 · 원격 표시 · 더블클릭 배포 1회 · `.vercelignore` 추가) |
| 3 | `check-secrets.py --all` · `security-audit.py --all` | ✅ (경고 1: 공개 연락처 이메일) |
| 4 | 새 훅: 미검토 커밋 / 푸시 / `commit -a` / add+commit 차단, 일반 명령 통과 | ✅ 8개 사례 |
| ⚠️ | 미확인 | Vercel이 프레임워크 없는 배포에서 모든 파일을 제공하는지 실제 배포로는 확인하지 않음(제외 장치는 어느 쪽이든 옳음) · Codex에 `--` 구분자 미적용(미검증) |
