# 2026-09-18 컨텍스트 미터가 옆 세션 값을 보여 주던 문제

> **Historical work log.** Written while the work was done (September 2026) and kept as a record; it describes the
> code as it was then, and details may differ from the current app. For how Agentty works today, see the
> [documentation](../../README.md#documentation).
>
> **작업 당시의 기록입니다.** 2026년 9월 작업 중에 쓴 문서로, 그때의 코드를 설명하며 현재 앱과 다를 수 있습니다.

상태: ⬜ 대기 · 🔧 진행 중 · ✅ 완료 · ⚠️ 부분 완료/제약

## 증상

- 셸 창에서 `claude`를 실행하자마자 Context가 **10%**로 표시되고, 첫 프롬프트를 보내면 **5%**로 바뀜

## 확인한 사실 (추측 아님)

| 항목 | 값 |
|---|---|
| 이 창의 셸 시작 | 17:12:22 (`launched_at_ms`는 창 생성 시각) |
| 같은 폴더의 다른 Claude 세션 | 마지막 기록 17:29:59, 마지막 요청 103,587 토큰 = 1M의 **10.4%** |
| 이 창의 `claude` 시작 → 첫 프롬프트 | 17:36:14 → 17:36:31 (트랜스크립트는 첫 프롬프트 때 생성됨) |

1. pid 레지스트리에 등록되기 전 한 번의 probe에서 `find_recent(cwd, since = 창 생성 시각)`이 같은 폴더의 **옆 세션** 트랜스크립트를 골라 10.4%를 표시
2. 이후 레지스트리가 진짜 세션 id를 알려 주지만 트랜스크립트가 아직 없어 `session_stats`가 `None` → `if stats.is_some()` 조건 때문에 **옆 세션 값이 그대로 남음**
3. 첫 프롬프트 뒤 자기 트랜스크립트가 생기면서 5%로 정상화

## 수정

| 항목 | 내용 |
|---|---|
| `agent_since_ms` | 에이전트가 포그라운드에 처음 보인 시각. `find_recent`의 기준을 창 생성 시각 대신 이 값으로 (오래된 셸 창에 나중에 친 에이전트) |
| `claude::owned_by_other_process` | 다른 실행 중인 Claude Code 프로세스가 레지스트리에 등록한 세션이면 "최근 트랜스크립트" 후보에서 제외 |
| 통계 교체 규칙 | 세션 id가 바뀌면 `None`이어도 교체(새 세션은 첫 프롬프트 전까지 비어 있음). 같은 세션에서 일시적으로 못 읽은 경우만 이전 값 유지 |

## 검증

| # | 항목 | 결과 |
|---|---|---|
| 1 | `claude::tests::a_neighbours_session_is_not_this_panes` | ✅ |
| 2 | fmt · clippy · test | ✅ |
| ⚠️ | 실제 앱에서 같은 폴더 두 세션으로 재현 확인 | 미확인 (빌드한 앱으로 직접 확인 필요) |
