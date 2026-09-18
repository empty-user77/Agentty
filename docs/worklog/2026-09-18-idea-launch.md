# 2026-09-18 아이디어 실현 모드 · 출시(Launch) 플러그인

상태: ⬜ 대기 · 🔧 진행 중 · ✅ 완료 · ⚠️ 부분 완료/제약

## 요청 요약

- 일반인도 Agentty로 **아이디어 → 개발 → 출시**까지 이어지게
- **아이디어 실현 모드**(+ 메뉴 · 시작 화면): 채팅에 아이디어/기획서 → 에이전트 오케스트레이션 개발 → 실시간 미리보기
- **출시 플러그인**: GitHub 커밋·푸시 → Vercel 배포 → 출시 완료 (딸각 수준)

## 설계

| 결정 | 내용 · 이유 |
|---|---|
| 아이디어 페이지 | 네이티브 페이지(`idea_view.rs`). 입력창은 한 줄이라 **채팅 방식**: Enter = 메시지 추가, 여러 줄 붙여넣기 = 문서 메시지(`TextInput::keep_pasted_lines`), 파일 첨부 버튼 · 페이지에 끌어다 놓기. 예시 문장 칩, 에이전트(Claude Code/Codex), 저장 위치 |
| 프로젝트 생성 | `agentty_bridge::idea` — `~/AgenttyProjects/<slug>/docs/idea/`(IDEA.md, attachments/, BUILD_GUIDE.md) + `.claude/settings.json`. 다른 파일은 만들지 않아 create-next-app 등이 폴더를 거부하지 않음 |
| 오케스트레이션 | 첫 프롬프트 + BUILD_GUIDE: PLAN.md → 첫 화면 빨리 띄우고 내장 브라우저(`browser_open`)로 열어 계속 새로고침 → 독립 작업은 서브에이전트 병렬 → `npm run build`/콘솔/스크린샷 검증 → 쉬운 말로 결과 설명 + 🚀 출시 안내 |
| 권한 | 새 프로젝트의 `.claude/settings.json`만 `acceptEdits` + npm/npx/node/mkdir/ls/cat/mv/cp/git init·add·commit/브라우저 도구 허용. 삭제·push·그 밖의 명령은 여전히 확인 |
| 출시 | 내장 플러그인 `launch`(Node). 첫 사용(+ 메뉴 "웹에 출시하기", 아이디어 프로젝트 시작) 때 자동 설치 → 에이전트 창에 🚀 버튼 |
| 로그인 | 공식 CLI 브라우저 로그인만 사용(`gh auth login --web`, `vercel login` 디바이스 코드). 코드는 패널에 크게 표시 + 클립보드 복사 + 브라우저 자동 열기. 토큰은 Agentty/플러그인이 보거나 저장하지 않음 |
| CLI 설치 | 없으면 Homebrew 없이 플러그인 데이터 폴더에 설치: gh = GitHub 릴리스 zip, vercel = npm |
| 안전장치 | .gitignore 보강(.env*, node_modules, .vercel …), 이미 추적/스테이징된 `.env` 있으면 저장 거부, 저장소 기본 비공개, 환경변수는 이름만 표시하고 값은 `vercel env add`에 stdin으로만 전달 |
| 배포 URL | `Aliased`(공개 도메인) 우선. 없으면 `vercel inspect`의 가장 짧은 `*.vercel.app` — 배포별 URL은 Vercel 배포 보호로 방문자에게 로그인 화면이 뜨기 때문 |
| 실패 시 | 짧은 설명 + 로그 + "다시 시도" + "에이전트에게 고쳐달라고 하기"(실패 명령과 로그 60줄을 프롬프트로) |

## 검증

| # | 항목 | 결과 |
|---|---|---|
| 1 | `cargo fmt --all -- --check` | ✅ |
| 2 | `cargo clippy --workspace --all-targets -- -D warnings` | ✅ |
| 3 | `cargo test --workspace` | ✅ app 86 / bridge 77 (신규 idea 5개) |
| 4 | `cargo build --release -p agentty-app` | ✅ |
| 5 | `python3 scripts/check-secrets.py --all` | ✅ |
| 6 | `node --test plugins/launch/test/plugin.test.mjs` | ✅ 18개 (파서, .env 가드, 가짜 gh/vercel + 실제 git으로 저장→배포→출시 완료 E2E) |
| 7 | 실제 gh 2.89 비대화형 로그인 출력 | ✅ 코드·URL 파싱 확인 (격리 설정 폴더, 로그인 완료 전 중단) |
| 8 | 실제 vercel 59.23 비대화형 로그인 출력 | ✅ 디바이스 코드 방식, 텔레메트리 URL이 아닌 로그인 URL 선택 확인 |
| 9 | 실제 앱(격리 데이터 폴더) | ✅ 시작 화면 카드, + 메뉴, 아이디어 페이지(메시지·문서·첨부) 캡처 확인 · Launch 첫 사용 자동 설치 + 플러그인 기동 |
| ⚠️ | 미확인 | 실제 계정으로 로그인 끝까지 · 실제 Vercel 배포 · "만들기 시작" 후 실제 에이전트 개발 전 과정 · Launch 패널 화면 캡처(화면 잠김) |

## 남은 것

- 커스텀 도메인 (Supabase 연동은 아래 절 참고 — 실계정 확인만 남음)
- 아이디어 페이지 여러 줄 편집기(현재는 붙여넣기로 여러 줄 입력)
- 진행 상황 시각화(PLAN.md 체크리스트를 패널에 표시)

## Supabase 연동 (✅ 구현 · ⚠️ 실계정 미확인)

### 확인한 CLI 동작 (supabase 2.117.0, npm 패키지 `supabase`)

| 상황 | 결과 |
|---|---|
| 파이프(TTY 없음)로 `supabase login` | `LegacyLoginMissingTokenError` — "non-TTY에서는 자동 로그인 불가, `--token` 또는 `SUPABASE_ACCESS_TOKEN` 필요" |
| 가짜 TTY + `--agent no --output-format text` | 로그인 링크 출력 (`https://supabase.com/dashboard/cli/login?session_id=…&token_name=…&public_key=…`) 후 브라우저에 표시되는 **인증 코드 입력 대기** |
| Node에서 `script -q /dev/null …` 직접 실행 | ❌ `script: tcgetattr/ioctl: Operation not supported on socket` — Node의 stdio 파이프는 소켓쌍이라 `script`가 거부 |
| `bash -c 'script -q /dev/null "$0" "$@" < <(cat 2>/dev/null)'` | ✅ `cat`이 진짜 파이프를 끼워 줌. 프로세스 치환이라 CLI가 끝나면 바로 종료(성공 0 / 조기 실패 코드 그대로), 취소 시 고아 프로세스 없음 |
| 미로그인 `projects list -o json` | exit 1 + stderr `Access token not provided…` → 로그인 여부 판정에 사용 |
| 플래그 | `projects create <name> --org-id --region --db-password`, `projects api-keys --project-ref [--reveal]`, `link --project-ref`, `db push [--yes]`, 전역 `--agent no` · `-o json` |

→ gh/vercel과 달리 **코드를 사용자가 입력**: 패널이 링크를 열고 코드 입력칸(`ui.input`)을 보여 주며, 입력값을 가짜 TTY 프로세스 stdin으로 전달.

### 구현

| 항목 | 내용 |
|---|---|
| 단계 위치 | `vercel-login` → **`supabase`** → `env` → `deploy`. 키를 쓴 직후 환경변수 단계가 다시 떠서 Vercel에 등록됨 |
| 표시 조건 | `@supabase/*` 의존성 · `supabase/` 폴더 · `…SUPABASE…` 환경변수 이름. 아니면 출시/배포 화면의 "데이터베이스 연결 (Supabase)" 버튼 |
| CLI | PATH 또는 `<dataDir>/tools`에 npm 설치 (`installNpmCli`로 vercel과 공용화) |
| 프로젝트 | 기존 프로젝트 선택(`ui.list`) 또는 새로 만들기 — 폴더 이름, 시간대에서 고른 지역(`Asia/Seoul` → `ap-northeast-2`), 조직이 여럿이면 `ui.choice`. 생성 뒤 `ACTIVE_HEALTHY`까지 대기. "다시 시도"가 두 번째 프로젝트를 만들지 않도록 생성된 ref를 기억 |
| 키 | `projects api-keys -o json`에서 `anon`(없으면 `publishable`)만. `service_role`/`secret`은 고르지 않음(`pickSupabasePublicKey`), `--reveal` 미사용 |
| 저장 위치 | `.gitignore` 보강 후 `.env.local`(0600). 프로젝트가 이미 쓰는 이름 우선, 없으면 프레임워크별(`NEXT_PUBLIC_` / `VITE_` / `PUBLIC_`). `.env.example`에는 이름만. `.env.local`이 로컬 Supabase를 가리키면 `.env.production`에 기록 |
| DB 비밀번호 | Launch가 만든 프로젝트는 무작위 생성, 기존 프로젝트는 마이그레이션 적용 시 한 번 입력. `.env.local`의 `SUPABASE_DB_PASSWORD`에만 저장, `link`/`db push`에는 환경변수로 전달, 환경변수 단계에서 기본 꺼짐(`LOCAL_ONLY_KEYS`), 패널·`projects.json`에 없음 |
| 마이그레이션 | `supabase/migrations/*.sql` 중 미적용분이 있으면 "데이터베이스 변경 적용" (`link` + `db push --yes`). "사이트 업데이트" 전에도 먼저 확인하고, 적용 후 업데이트를 이어서 진행 |
| 에이전트 | "에이전트에게 데이터베이스 연결 맡기기": supabase-js 클라이언트 + RLS가 켜진 마이그레이션 작성 요청, service_role 금지 |
| 오류 | 무료 플랜 한도 · 조직 없음 · 아직 시작 중 · DB 비밀번호 불일치 · 로그인 미완료는 쉬운 말로 안내 |
| BUILD_GUIDE | 데이터 절에 환경변수 이름, 마이그레이션 위치, RLS, service_role 금지 추가 |
| 기타 | `.gitignore` 필수 줄에 `supabase/.temp` 추가. 플러그인 0.2.0 (내장 업데이트 감지용). 환경변수 단계: `state.saved`를 갱신하지 않아 추가/건너뛰기 후 같은 단계가 다시 뜨던 문제 수정, 로컬 전용 값은 기본 선택 해제 |

### 검증

| # | 항목 | 결과 |
|---|---|---|
| 1 | `node --test plugins/launch/test/plugin.test.mjs` | ✅ 28개 (신규 10: 파서 8 + E2E 2) |
| 2 | E2E ① 로그인 코드 입력 → 새 프로젝트 → 키 기록 → 마이그레이션 → 환경변수 단계 | ✅ 가짜 `supabase` CLI + 실제 `script` 가짜 TTY. `.env.local` 0600, service_role 미기록, 패널·상태 파일에 키/비밀번호 없음, `db push`에 비밀번호는 환경변수로만 |
| 3 | E2E ② 기존 프로젝트 선택 → DB 비밀번호 입력 → 마이그레이션 | ✅ |
| ⚠️ | 미확인 | 실제 계정으로 로그인 완료 · `projects list`/`create`/`api-keys`의 실제 JSON(두 가지 형태 모두 허용하도록 파싱) · 실제 `link`/`db push` 비대화형 동작 · `projects create`의 `--db-password`는 CLI가 문서화한 방식이라 인자로 전달(같은 Mac의 다른 사용자가 실행 순간 `ps`로 볼 수 있음) |
