---
title: agentty-gpui
description: Agentty 전용 UI 프레임워크. GPU로 그리는 Rust UI로 macOS·Windows·Linux를 지원하며, 크레이트 20개로 crates.io에 배포됩니다.
---

agentty-gpui는 Agentty 전용 UI 프레임워크입니다. Zed의 gpui를 기반으로 만들었습니다.

Agentty의 화면을 GPU로 그리는 데 필요한 것을 모두 담고 있습니다. 엘리먼트와 레이아웃, 창과 메뉴, 키보드·마우스 입력, 텍스트 셰이핑과 글꼴, 이미지와 SVG, 그리고 macOS·Windows·Linux 백엔드와 렌더러가 들어 있습니다. Agentty에 필요한 수정과 기능은 이 저장소에 바로 넣고, 다른 Rust 앱도 똑같이 가져다 쓸 수 있도록 [crates.io](https://crates.io/crates/agentty-gpui)에 크레이트 20개로 배포합니다.

- 저장소: [github.com/empty-user77/agentty-gpui](https://github.com/empty-user77/agentty-gpui)
- 크레이트: [crates.io/crates/agentty-gpui](https://crates.io/crates/agentty-gpui)
- API 문서: [docs.rs/agentty-gpui](https://docs.rs/agentty-gpui)
- 라이선스: Apache License 2.0
- Rust: 2024 에디션, 툴체인 1.98 (Agentty와 같은 버전)

## 시작하기

`Cargo.toml`에 두 크레이트를 넣습니다. 프레임워크 본체인 `agentty-gpui`와, 실행 중인 OS의 백엔드를 띄우는 `agentty-gpui-platform`입니다. 라이브러리 이름은 `gpui`, `gpui_platform`이므로 코드에서는 `use gpui::...`로 씁니다.

```toml
[dependencies]
gpui = { package = "agentty-gpui", version = "0.1" }
gpui_platform = { package = "agentty-gpui-platform", version = "0.1" }
```

```rust
fn main() {
    gpui_platform::application().run(|cx: &mut gpui::App| {
        // 창 열기, 키 바인딩 등
    });
}
```

플랫폼별로 켜야 할 기능이 있습니다.

| 플랫폼 | 설정 |
|---|---|
| macOS | `agentty-gpui-platform`의 `runtime_shaders`를 켜면 Metal 셰이더를 실행할 때 컴파일합니다. Xcode의 Metal 툴체인 없이 빌드됩니다. 끄면 빌드할 때 컴파일하므로 툴체인이 필요합니다(`xcodebuild -downloadComponent MetalToolchain`). |
| Windows | 추가 설정이 없습니다. 기본 기능 `windows-manifest`가 앱 매니페스트를 실행 파일에 넣습니다. |
| Linux | 두 크레이트 모두 `wayland`나 `x11`(또는 둘 다)를 켭니다. |

## 기능 플래그

`agentty-gpui`의 기능입니다. 기본값은 `font-kit`, `wayland`, `x11`, `windows-manifest`입니다.

| 기능 | 하는 일 |
|---|---|
| `font-kit` | 시스템 글꼴을 찾아 읽습니다(기본) |
| `wayland`, `x11` | Linux 디스플레이 서버 지원(기본) |
| `windows-manifest` | Windows 앱 매니페스트를 실행 파일에 넣습니다(기본) |
| `screen-capture` | 화면 캡처 |
| `inspector` | 실행 중인 화면의 엘리먼트를 살펴보는 인스펙터 |
| `test-support` | 테스트용 플랫폼과 도우미 |
| `leak-detection` | 엔티티 핸들을 추적해 테스트에서 누수를 잡습니다. 빌드할 때 `GPUI_LEAK_DETECTION`을 설정해도 켜집니다 |
| `bench-support` | 벤치마크와 프로파일러(`bench`는 같은 기능의 예전 이름) |
| `profiler` | 프레임 시간 히스토그램 |
| `stacker` | 깊은 재귀에서 스택을 늘려 씁니다 |
| `hot-reload` | 개발 중 코드 핫 리로드 |

`agentty-gpui-platform`은 같은 이름의 기능을 각 백엔드로 넘깁니다(`font-kit`, `inspector`, `screen-capture`, `test-support`, `bench-support`, `wayland`, `x11`, `runtime_shaders`). 기본값은 `multithreaded`입니다.

## 크레이트

모두 같은 버전으로 배포되며, 빌드할 때는 대상 플랫폼의 크레이트만 컴파일됩니다. 링크는 각 크레이트의 crates.io 페이지입니다.

### 프레임워크

| 크레이트 | 라이브러리 이름 | 하는 일 | 의존 |
|---|---|---|---|
| [agentty-gpui](https://crates.io/crates/agentty-gpui) | `gpui` | 프레임워크 본체. 엘리먼트, flexbox 레이아웃(Taffy), 창, 입력, 액션과 키 바인딩, 텍스트, 이미지·SVG, 비동기 실행기 | collections, http-client, macros, refineable, scheduler, shared-string, sum-tree, util, bench-metrics(선택) |
| [agentty-gpui-platform](https://crates.io/crates/agentty-gpui-platform) | `gpui_platform` | 현재 OS의 백엔드를 골라 앱을 시작합니다(`gpui_platform::application()`) | agentty-gpui, macos, windows, linux, wgpu(선택) |
| [agentty-gpui-macros](https://crates.io/crates/agentty-gpui-macros) | `gpui_macros` | 프레임워크의 derive·attribute 매크로 | — |

### 플랫폼 백엔드와 렌더러

| 크레이트 | 라이브러리 이름 | 하는 일 | 의존 |
|---|---|---|---|
| [agentty-gpui-macos](https://crates.io/crates/agentty-gpui-macos) | `gpui_macos` | macOS 백엔드. AppKit 창과 메뉴, 입력, 알림, 화면 캡처(선택) | agentty-gpui, apple, collections, scheduler, util |
| [agentty-gpui-apple](https://crates.io/crates/agentty-gpui-apple) | `gpui_apple` | Apple 플랫폼이 함께 쓰는 Metal 렌더러와 Core Text 텍스트 | agentty-gpui, collections, util |
| [agentty-gpui-windows](https://crates.io/crates/agentty-gpui-windows) | `gpui_windows` | Windows 백엔드. Direct3D 11 렌더러, DirectWrite 텍스트, DirectComposition | agentty-gpui, collections, scheduler, util |
| [agentty-gpui-linux](https://crates.io/crates/agentty-gpui-linux) | `gpui_linux` | Linux 백엔드. Wayland와 X11, 키보드 배치(xkbcommon), 데스크톱 포털 | agentty-gpui, collections, http-client, util, wgpu(선택) |
| [agentty-gpui-wgpu](https://crates.io/crates/agentty-gpui-wgpu) | `gpui_wgpu` | Linux에서 쓰는 wgpu 렌더러 | agentty-gpui, collections, util |

### 기반 라이브러리

| 크레이트 | 라이브러리 이름 | 하는 일 | 의존 |
|---|---|---|---|
| [agentty-gpui-util](https://crates.io/crates/agentty-gpui-util) | `gpui_util` | 작은 공용 도우미 | — |
| [agentty-gpui-shared-string](https://crates.io/crates/agentty-gpui-shared-string) | `gpui_shared_string` | 복사 없이 공유하는 문자열 `SharedString` | — |
| [agentty-gpui-collections](https://crates.io/crates/agentty-gpui-collections) | `collections` | 해시 맵과 해시 셋 타입 | util |
| [agentty-gpui-sum-tree](https://crates.io/crates/agentty-gpui-sum-tree) | `sum_tree` | 리스트와 텍스트 밑에 깔리는 B-트리. 동시성에 강합니다 | ztracing |
| [agentty-gpui-scheduler](https://crates.io/crates/agentty-gpui-scheduler) | `scheduler` | 비동기 작업 스케줄러 | — |
| [agentty-gpui-refineable](https://crates.io/crates/agentty-gpui-refineable) | `refineable` | 스타일을 부분만 덮어쓰게 하는 `Refineable` 트레이트 | derive-refineable |
| [agentty-gpui-derive-refineable](https://crates.io/crates/agentty-gpui-derive-refineable) | `derive_refineable` | `Refineable`의 derive 매크로 | — |
| [agentty-gpui-http-client](https://crates.io/crates/agentty-gpui-http-client) | `http_client` | HTTP 클라이언트 인터페이스. URL에서 이미지를 불러올 때 씁니다 | — |
| [agentty-gpui-zlog](https://crates.io/crates/agentty-gpui-zlog) | `zlog` | 로깅 | collections |
| [agentty-gpui-ztracing](https://crates.io/crates/agentty-gpui-ztracing) | `ztracing` | 트레이싱. Tracy 프로파일러 연동(선택) | ztracing-macro, zlog(선택) |
| [agentty-gpui-ztracing-macro](https://crates.io/crates/agentty-gpui-ztracing-macro) | `ztracing_macro` | 트레이싱 attribute 매크로 | — |
| [agentty-gpui-bench-metrics](https://crates.io/crates/agentty-gpui-bench-metrics) | `bench_metrics` | 성능 카운터 | — |

iOS와 웹 백엔드는 넣지 않았습니다. Agentty가 macOS, Windows, Linux용으로 나오기 때문입니다. 원본의 HTTP 클라이언트가 필요한 `image`, `image_gallery` 예제도 빠져 있습니다.

## crates.io 의존성

crates.io에 올리는 크레이트는 crates.io의 크레이트에만 의존할 수 있습니다. 원본이 Git이나 포크에서 가져오는 의존성은 crates.io 릴리스로 바꿨습니다.

| 크레이트 | 여기서 쓰는 버전 | 원본 쪽이 더 가진 것 |
|---|---|---|
| `async-task` | 4.7.1 | 작업 waker vtable 식별 수정 |
| `calloop` (Linux) | 0.14.5 | 루프마다 idle 콜백을 약 10ms로 제한 |
| `khronos-egl` (Linux) | 6.0.0 | 실패한 EGL 호출이 성공을 보고할 때 패닉하지 않음 |
| `wgpu`, `naga` (Linux) | 30.0.2 | 더 새 API(`QueueDescriptor`, `has_srgb_suffix`) |
| `zed-font-kit`, `zed-scap`, `zed-xim` | crates.io 릴리스 | 그 뒤의 작은 수정 |
| `oo7` (Linux) | 0.6 | 프롬프트 스트림이 끝날 때의 오류 |
| `windows-capture` (Windows, `screen-capture`) | 1.4.4, `Cargo.lock`에 고정 | 없음. 1.5는 `zed-scap`과 빌드되지 않으므로 `screen-capture`를 켜는 쪽도 1.4.4로 고정해야 합니다 |

## 원본에서 바꾼 파일

바꾼 파일은 크레이트 매니페스트를 포함해 모두 맨 위에 `Modified for Agentty` 주석이 있습니다.

- `crates/gpui/src/style.rs`: 테스트가 `#[perf]` 대신 `#[test]`를 씁니다.
- `crates/gpui_apple/build.rs`: `../gpui`가 없을 때(배포된 크레이트) 함께 넣은 `gpui-src/` 사본을 읽습니다. 스크립트가 이 사본을 `crates/gpui/src`와 같게 유지합니다.
- `crates/gpui_wgpu/src/{wgpu_context,wgpu_renderer,wgpu_atlas}.rs`: wgpu 30.0.2 API에 맞춤.

크레이트 매니페스트는 `scripts/apply_agentty.py`가 씁니다. 패키지 이름을 `agentty-gpui-*`로 바꾸고, 워크스페이스 버전을 넣고, iOS·웹을 빼고, Git 의존성을 crates.io로 바꿉니다. CI는 이 스크립트의 결과가 커밋되지 않았으면 실패합니다.

## 업데이트 방식

브랜치는 두 개입니다.

- **`main`**: 배포되는 코드. Agentty를 위한 변경이 모두 여기에 있습니다.
- **`upstream`**: 기반이 된 원본 그대로의 코드. 원본 릴리스마다 커밋 하나씩 쌓이고, 어느 릴리스인지는 `zed/SNAPSHOT.md`에 적혀 있습니다.

새 원본 릴리스를 받는 순서입니다.

```sh
scripts/sync-upstream.sh v1.29.0   # upstream에 새 스냅숏
git merge upstream                 # main으로 병합. 충돌은 우리가 바꾼 파일에서만 납니다
python3 -I scripts/apply_agentty.py
```

병합 중에 크레이트 매니페스트가 충돌하면 원본 쪽을 받고(`git checkout --theirs <파일>`) 스크립트를 다시 돌립니다.

## 테스트

```sh
cargo nextest run --workspace --features agentty-gpui-apple/runtime_shaders
cargo test --workspace --doc --features agentty-gpui-apple/runtime_shaders
```

- nextest를 씁니다. 몇몇 테스트가 프로세스 전역 상태를 읽어서, `cargo test`처럼 나란히 돌리면 실패합니다.
- CI는 `--profile ci`(`.config/nextest.toml`)로 실패한 테스트를 두 번까지 다시 돌립니다. 80ms 폴링을 재는 벤치 테스트가 바쁜 러너에서 늦을 수 있기 때문입니다.
- Linux CI는 wgpu 렌더러 테스트에 Mesa의 소프트웨어 Vulkan 드라이버를 씁니다.

## 배포 절차

1. `scripts/apply_agentty.py`의 `VERSION`을 올리고 스크립트를 다시 돌린 뒤 푸시합니다. `cargo publish --workspace --dry-run`을 포함해 CI가 통과해야 합니다.
2. GitHub Actions의 **Agentty** 워크플로를 돌립니다. 이 체크아웃으로 Agentty를 macOS, Windows, Linux에서 빌드하고 테스트합니다.
3. `cargo publish --workspace`로 의존 순서대로 배포합니다.
4. 릴리스 태그를 붙입니다(`git tag v0.1.0 && git push origin v0.1.0`).

## 문제 알리기

agentty-gpui의 버그나 바라는 변경은 [agentty-gpui 이슈](https://github.com/empty-user77/agentty-gpui/issues)에, Agentty의 버그는 [Agentty 이슈](https://github.com/empty-user77/Agentty/issues)에 남겨 주세요. 운영체제마다 Agentty가 어떻게 다른지는 [플랫폼](/docs/platforms)을 보세요.
