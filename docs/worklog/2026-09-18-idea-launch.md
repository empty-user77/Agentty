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

## 후속 (같은 날)

### 코드 리뷰 반영 (Supabase)

| 지적 | 수정 |
|---|---|
| "새 프로젝트 만들기" 더블클릭 시 프로젝트가 2개 생성될 수 있음 (SDK가 이벤트를 동시에 처리) | `supabaseStep`이 실행 중이면 두 번째 이벤트 무시 |
| 프로젝트 생성 직후 저장이 실패하면 생성된 DB 비밀번호 유실 | 비밀번호를 **생성 전에** `.env.local`에 저장, 생성 실패 시 이전 값 복원(없었으면 파일도 남기지 않음). 비밀번호는 `supabase.mjs` 밖으로 나오지 않음 |
| DB 비밀번호 오류 뒤에는 "사이트 업데이트"가 이어지지 않음 | "다시 시도"가 래퍼(`applySupabaseMigrations`)를 거치도록 `runStep`에 `retry` 추가, 실패 시 `thenUpdate` 유지 |
| CLI 오류 출력이 그대로 패널·에이전트 프롬프트로 전달됨 | 알고 있는 비밀번호는 오류 문자열에서 `***`로 마스킹 |
| ⚠️ 남김 | `projects create --db-password`는 인자로 전달 (CLI 문서화 방식, 환경변수 지원 여부 미확인) |

플러그인 0.2.1 · `node --test` 29개 ✅ (생성 거부 시 평이한 안내 + 비밀번호 미잔류 + 더블클릭 무시 테스트 추가)

### 아이디어 모드 시작 시 `Invalid MCP configuration … ENAMETOOLONG`

- 원인: `claude … --settings <json> --mcp-config <json> '<프롬프트>'`. `--mcp-config`는 값을 여러 개 받는 옵션이라 뒤따르는 프롬프트를 두 번째 설정 파일 경로로 읽음. 브라우저 도구가 켜진 상태에서 프롬프트로 시작하는 모든 Claude 창에 해당 (`Start::Prompt`)
- 재현: `claude -p --settings '{}' --mcp-config '{"mcpServers":{}}' 'say hi'` → `MCP config file not found: …/say hi`. 순서를 바꾸면 정상 파싱
- 수정: `--mcp-config`를 `--settings` 앞으로 (`launch.rs`), 회귀 테스트 `claude_prompt_is_not_swallowed_by_mcp_config`

### 시작 화면 개편

| 항목 | 내용 |
|---|---|
| 구조 | 헤더(로고 · 이름 · 태그라인) → **아이디어 실현하기** 전체 폭 카드 → **시작하기** 카드 그리드(터미널 · Claude Code · Codex · 설치된 주요 에이전트) → **최근 세션**(6개, 클릭하면 이어서 실행) · **둘러보기**(웹에 출시하기 · 세션 연결 · AI 사용량 · 플러그인) → 푸터 |
| 카드 | 브랜드 색 사각 타일(`brand::tile`) + 이름 + 한 줄 설명. 폭에 맞춰 1~4열, 보이지 않는 채움 카드로 마지막 줄도 같은 열 폭 |
| 문구 | 아이디어 카드 "아이디어만 가져오세요. 만들고 출시하는 건 Agentty가 도와드립니다."(한 줄, 넘치면 말줄임) · 터미널 "자유롭게 시작하세요." · 에이전트 "{제작사}의 코딩 에이전트"(`AgentCli::maker`: Gemini/Antigravity = Google) · 헤더의 "바로 아이디어를 실현시켜 보세요!" 제거 |
| 새 워크스페이스 | 같은 화면에 이름 입력 · 그룹 선택, 취소/터미널로 돌아가기는 헤더 오른쪽 버튼 |
| 첫 실행 | 웹사이트를 내장 브라우저로 여는 투어 제거 (`welcome_shown` 설정 삭제) |
| 검증 | ✅ 디버그 스냅샷으로 넓은 창(4열)·좁은 창(2열) 확인 · fmt · clippy · test(app 88 / bridge 78) · 릴리스 빌드 · 시크릿 스캔 |

### 아이디어 프로젝트 기본값 · Launch 패널 제목

| 항목 | 내용 |
|---|---|
| auto 모드 | 아이디어 프로젝트(`docs/idea/BUILD_GUIDE.md`가 있는 폴더)에서 Agentty가 띄우는 Claude Code는 `--permission-mode auto`로 시작 — 첫 실행뿐 아니라 그 폴더에서 나중에 여는 탭·이어하기도 동일 |
| 확인한 사실 | `--setting-sources project`로 사용자 설정을 빼고 시험: 프로젝트 `.claude/settings.json`의 `"defaultMode": "auto"`는 **무시됨**(`permissionMode: default`), `acceptEdits`는 적용됨, `--permission-mode auto` 플래그는 적용됨 → 명령줄로만 켤 수 있음 |
| 구버전 대비 | 에이전트 감지 때 `claude --help`에 `"auto"`가 있는지 확인(`agents::claude_auto_mode`). 모르는 버전에 플래그를 주면 실행 자체가 실패하므로 그때는 기존 `acceptEdits` + 허용 목록으로 동작 |
| 그룹 | 아이디어로 만든 워크스페이스는 **Agentty Idea** 그룹에 들어감. 이름으로 찾고, 없으면 만들며, 접혀 있으면 펼침 |
| Launch 제목 깨짐 | 플러그인 패널의 `row` 안 텍스트가 최소 폭으로 줄어 한글이 한 글자씩 세로로 표시("출/시")되던 문제 — `row` 안의 텍스트가 버튼이 남긴 폭을 차지하도록 수정(`plugin_panel.rs`). 디버그 스냅샷으로 전/후 확인 |
| ⚠️ 미확인 | auto 모드를 쓸 수 없는 계정에서 플래그를 줬을 때의 Claude Code 동작 · Codex에는 해당 모드 없음(변경 없음) |
| 설정 | 설정 > 일반 **아이디어 모드 사용하기**(기본 ON, `Settings::idea_mode`). OFF면 시작 화면 배너 · + 메뉴 · 명령 팔레트의 아이디어 항목을 숨김 |
| 아이디어 페이지 문구 | "새 프로젝트 폴더와 워크스페이스가 만들어지고…" 안내 제거 · 입력창 안내를 "영감을 최대한 많이 전달해 주세요. 몇 마디든, 몇 개의 파일이든 좋습니다."로 교체 |

### 프롬프트 · 규칙 · 스킬은 영어로만

전수 조사(추적 파일 전체에서 한글 검색, 워크로그 제외) 결과와 조치:

| 위치 | 내용 | 조치 |
|---|---|---|
| `agentty-bridge/src/idea.rs` | 아이디어 첫 프롬프트의 한국어판(`PROMPT_KO`) | 삭제. 영어 프롬프트 하나 + `Talk to me in {language}` (`idea::language_name`) |
| `i18n.rs` | 플러그인 개발 프롬프트 `plugins.ai_prompt` / `plugins.ai_prompt_ask` (4개 언어) | 번역 표에서 제거 → `plugins_page.rs`의 영어 상수, 대화 언어 지시 추가 |
| `plugins/launch/main.mjs` | 한국어 `fixPrompt` · `sbPrompt` | 삭제(영어로 폴백). 패널 UI 번역은 유지 · 0.2.2 |
| `plugins/cosmica/main.mjs` | ko/ja/zh `continuePrompt` · `insertPrompt` · `summaryPrompt` | 삭제. 요약 프롬프트는 "쓰던 언어로 작성" 지시가 이미 있음 · 1.0.2 |
| `.claude/skills/release/SKILL.md` | 한국어 체크리스트 예시, 설명의 한국어 트리거 단어 | 영어로 |
| 대상 아님 | UI 번역(`i18n.rs`, 플러그인 STRINGS), CJK 처리 테스트 픽스처, 글꼴 미리보기, 언어 이름, README 태그라인 | 유지 |

`CLAUDE.md` 프로젝트 규칙에 추가: 코드·번들 문서의 프롬프트/규칙/스킬은 영어로만 쓰고 번역 표에 넣지 않는다. 번들 문서(`docs/plugins/*.md`, 플러그인 템플릿, BUILD_GUIDE, 세션 연결·핸드오프·하네스 프롬프트)는 이미 영어였음.

### "도구 설치" 오류 — `env: node: No such file or directory`

| 항목 | 내용 |
|---|---|
| 확인한 사실 | 로그인 셸 PATH에 공백이 있는 항목(`…/Application Support/JetBrains/Toolbox/scripts`)이 있음. 호스트는 셸 출력에서 "`/`가 있고 공백이 없는 줄"을 PATH로 골랐기 때문에 PATH 전체를 버리고 앱의 최소 PATH로 폴백 → node 자체는 nvm 폴백으로 찾지만 플러그인 PATH에는 node 폴더가 없어 `npm`(`#!/usr/bin/env node`)이 실패. `run_in_login_shell`은 출력 끝 600자만 돌려주므로 긴 PATH는 잘리기도 함 |
| 호스트 수정 | `plugins/process.rs`: 마커 줄 다음 줄을 PATH로 읽음(`echo <marker>; printenv PATH`, 모든 셸에서 콜론 구분·전체 길이). 플러그인을 실행하는 Node.js의 폴더를 PATH 맨 앞에 추가(`with_dir_first`) |
| 플러그인 수정 | `lib/exec.mjs` `pathWithExtras`: `process.execPath`의 폴더를 PATH 맨 앞에 — 구버전 호스트에서도 동작. Launch 0.2.3 |
| 검증 | ✅ `PATH=/usr/bin:/bin:/usr/sbin:/sbin`(node 없음)에서 nvm node로 `installVercel` 실제 실행 → 설치 성공, `vercel --version` 59.23.1 · 단위 테스트 추가(호스트 2, 플러그인 1) |

### 하네스 먼저 — ECC에서 필요한 에이전트·스킬 설치

| 항목 | 내용 |
|---|---|
| 방식 | Agentty는 ECC를 **포함하지 않음**. 빌드 가이드의 0단계에서 에이전트가 아이디어를 읽은 뒤 ECC(github.com/affaan-m/ECC, MIT)를 프로젝트 안 `.agentty-ecc`로 받아 필요한 것만 `.claude/agents` · `.claude/skills` · `.claude/rules`에 설치하고, `docs/idea/HARNESS.md`에 ECC 커밋과 설치 목록·이유를 남긴 뒤 다운로드를 지움. 실패(오프라인)하면 건너뛰고 PLAN.md에 기록 |
| 다운로드 | `git clone --depth 1 --filter=blob:none --sparse …` + `sparse-checkout set agents skills rules` — 실제 저장소로 확인: 5초, 10MB, 에이전트 68 · 스킬 292 · 규칙 23 |
| 안전 규칙 | Markdown 정의만 설치(스킬은 `SKILL.md`와 `.md`만 — 확인 결과 ECC 스킬에 스크립트 파일 124개 포함). 설치 스크립트·훅 정의·MCP 설정·settings·실행 파일은 복사/실행 금지, 홈 폴더 설치 금지, 에이전트 4–8 · 스킬 5–10개로 제한 |
| 권한 | auto 모드가 아닐 때를 위해 위 네 개 명령을 프로젝트 허용 목록에 정확한 문자열로 추가 |
| ⚠️ 남은 선택 | 기본 브랜치 최신을 받음(커밋은 HARNESS.md에 기록). 외부 저장소 내용이 auto 모드 에이전트의 지침이 되므로, 릴리스 태그나 특정 커밋으로 고정할지는 결정 필요 · 실제 아이디어 프로젝트로 전 과정 실행은 미확인 |

### Launch 체크리스트 색

- 플러그인 UI의 목록 항목에 `tone`(neutral · info · success · warning · error, 배지와 같은 값) 추가 — 아이콘 색을 정함 (`plugins/ui.rs`, `plugin_panel.rs`, `docs/plugins/protocol.md`, Node SDK 주석)
- Launch 0.2.4: 완료 = 초록 체크, 진행 중 = 파랑, 실패 = 빨강, 대기 = 회색
- 검증: ✅ 디버그 스냅샷(완료된 "도구" · "GitHub 로그인"이 초록) · 브리지 파싱 테스트 · 플러그인 E2E에 단계 tone 검증 추가
