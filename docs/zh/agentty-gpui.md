---
title: agentty-gpui
description: Agentty 专用的 UI 框架：用 GPU 绘制的 Rust UI，支持 macOS、Windows 和 Linux，以 20 个 crate 发布在 crates.io 上。
---

agentty-gpui 是 Agentty 专用的 UI 框架，基于 Zed 的 gpui 打造。

它包含 Agentty 用 GPU 绘制界面所需的一切：元素与布局、窗口与菜单、键盘和鼠标输入、文字排版与字体、图片与 SVG，以及 macOS、Windows、Linux 的后端和渲染器。Agentty 需要的修复和功能直接加入这里，并以 20 个 crate 发布在 [crates.io](https://crates.io/crates/agentty-gpui) 上，任何 Rust 应用都能以同样的方式使用。

- 仓库：[github.com/empty-user77/agentty-gpui](https://github.com/empty-user77/agentty-gpui)
- Crate：[crates.io/crates/agentty-gpui](https://crates.io/crates/agentty-gpui)
- API 文档：[docs.rs/agentty-gpui](https://docs.rs/agentty-gpui)
- 许可证：Apache License 2.0
- Rust：2024 版次，工具链 1.98（与 Agentty 相同）

## 快速开始

在 `Cargo.toml` 中加入两个 crate：框架本体 `agentty-gpui`，以及为当前操作系统启动后端的 `agentty-gpui-platform`。它们的库名是 `gpui` 和 `gpui_platform`，所以代码里写 `use gpui::...`。

```toml
[dependencies]
gpui = { package = "agentty-gpui", version = "0.1" }
gpui_platform = { package = "agentty-gpui-platform", version = "0.1" }
```

```rust
fn main() {
    gpui_platform::application().run(|cx: &mut gpui::App| {
        // 打开窗口、绑定按键……
    });
}
```

各平台需要开启的内容：

| 平台 | 设置 |
|---|---|
| macOS | 开启 `agentty-gpui-platform` 的 `runtime_shaders` 后，Metal 着色器在启动时编译，构建不需要 Xcode 的 Metal 工具链。关闭时在构建时编译，需要该工具链（`xcodebuild -downloadComponent MetalToolchain`）。 |
| Windows | 无需额外设置。默认的 `windows-manifest` 功能会把应用清单嵌入可执行文件。 |
| Linux | 为两个 crate 开启 `wayland` 和/或 `x11`。 |

## 功能开关

`agentty-gpui` 的功能。默认开启 `font-kit`、`wayland`、`x11`、`windows-manifest`。

| 功能 | 作用 |
|---|---|
| `font-kit` | 查找并加载系统字体（默认） |
| `wayland`、`x11` | Linux 显示服务器支持（默认） |
| `windows-manifest` | 把 Windows 应用清单嵌入可执行文件（默认） |
| `screen-capture` | 屏幕捕获 |
| `inspector` | 查看运行中界面元素的检查器 |
| `test-support` | 测试用平台和辅助工具 |
| `leak-detection` | 跟踪实体句柄，在测试中发现泄漏；构建时设置 `GPUI_LEAK_DETECTION` 也会开启 |
| `bench-support` | 基准测试和性能分析器（`bench` 是它的旧名称） |
| `profiler` | 帧时间直方图 |
| `stacker` | 深度递归时扩展栈 |
| `hot-reload` | 开发时的代码热重载 |

`agentty-gpui-platform` 会把同名功能传给各个后端（`font-kit`、`inspector`、`screen-capture`、`test-support`、`bench-support`、`wayland`、`x11`、`runtime_shaders`），默认开启 `multithreaded`。

## Crate 一览

所有 crate 版本一致，构建时只会编译目标平台需要的 crate。链接指向各 crate 的 crates.io 页面。

### 框架

| Crate | 库名 | 作用 | 依赖 |
|---|---|---|---|
| [agentty-gpui](https://crates.io/crates/agentty-gpui) | `gpui` | 框架本体：元素、flexbox 布局（Taffy）、窗口、输入、动作与快捷键绑定、文字、图片与 SVG、异步执行器 | collections、http-client、macros、refineable、scheduler、shared-string、sum-tree、util、bench-metrics（可选） |
| [agentty-gpui-platform](https://crates.io/crates/agentty-gpui-platform) | `gpui_platform` | 为当前操作系统选择后端并启动应用（`gpui_platform::application()`） | agentty-gpui、macos、windows、linux、wgpu（可选） |
| [agentty-gpui-macros](https://crates.io/crates/agentty-gpui-macros) | `gpui_macros` | 框架的 derive 和属性宏 | — |

### 平台后端与渲染器

| Crate | 库名 | 作用 | 依赖 |
|---|---|---|---|
| [agentty-gpui-macos](https://crates.io/crates/agentty-gpui-macos) | `gpui_macos` | macOS 后端：AppKit 窗口和菜单、输入、通知、屏幕捕获（可选） | agentty-gpui、apple、collections、scheduler、util |
| [agentty-gpui-apple](https://crates.io/crates/agentty-gpui-apple) | `gpui_apple` | Apple 平台共用的 Metal 渲染器和 Core Text 文字 | agentty-gpui、collections、util |
| [agentty-gpui-windows](https://crates.io/crates/agentty-gpui-windows) | `gpui_windows` | Windows 后端：Direct3D 11 渲染器、DirectWrite 文字、DirectComposition | agentty-gpui、collections、scheduler、util |
| [agentty-gpui-linux](https://crates.io/crates/agentty-gpui-linux) | `gpui_linux` | Linux 后端：Wayland 和 X11、键盘布局（xkbcommon）、桌面门户 | agentty-gpui、collections、http-client、util、wgpu（可选） |
| [agentty-gpui-wgpu](https://crates.io/crates/agentty-gpui-wgpu) | `gpui_wgpu` | Linux 上使用的 wgpu 渲染器 | agentty-gpui、collections、util |

### 基础库

| Crate | 库名 | 作用 | 依赖 |
|---|---|---|---|
| [agentty-gpui-util](https://crates.io/crates/agentty-gpui-util) | `gpui_util` | 小型通用辅助工具 | — |
| [agentty-gpui-shared-string](https://crates.io/crates/agentty-gpui-shared-string) | `gpui_shared_string` | 无需复制即可共享的字符串 `SharedString` | — |
| [agentty-gpui-collections](https://crates.io/crates/agentty-gpui-collections) | `collections` | 哈希映射和哈希集合类型 | util |
| [agentty-gpui-sum-tree](https://crates.io/crates/agentty-gpui-sum-tree) | `sum_tree` | 列表和文字底层的 B 树，适合并发 | ztracing |
| [agentty-gpui-scheduler](https://crates.io/crates/agentty-gpui-scheduler) | `scheduler` | 异步任务调度器 | — |
| [agentty-gpui-refineable](https://crates.io/crates/agentty-gpui-refineable) | `refineable` | 让样式可以部分覆盖的 `Refineable` trait | derive-refineable |
| [agentty-gpui-derive-refineable](https://crates.io/crates/agentty-gpui-derive-refineable) | `derive_refineable` | 它的 derive 宏 | — |
| [agentty-gpui-http-client](https://crates.io/crates/agentty-gpui-http-client) | `http_client` | HTTP 客户端接口，用于从 URL 加载图片 | — |
| [agentty-gpui-zlog](https://crates.io/crates/agentty-gpui-zlog) | `zlog` | 日志 | collections |
| [agentty-gpui-ztracing](https://crates.io/crates/agentty-gpui-ztracing) | `ztracing` | 追踪，可选接入 Tracy 性能分析器 | ztracing-macro、zlog（可选） |
| [agentty-gpui-ztracing-macro](https://crates.io/crates/agentty-gpui-ztracing-macro) | `ztracing_macro` | 它的属性宏 | — |
| [agentty-gpui-bench-metrics](https://crates.io/crates/agentty-gpui-bench-metrics) | `bench_metrics` | 性能计数器 | — |

没有包含 iOS 和 Web 后端，因为 Agentty 面向 macOS、Windows 和 Linux。需要原始 HTTP 客户端的 `image` 和 `image_gallery` 示例也没有包含。

## crates.io 依赖

crates.io 上的 crate 只能依赖 crates.io 上的 crate。原始代码从 Git 或分叉获取的依赖，这里换成了 crates.io 的发布版本：

| Crate | 这里使用的版本 | 原始版本多出的内容 |
|---|---|---|
| `async-task` | 4.7.1 | 任务 waker vtable 标识的修复 |
| `calloop`（Linux） | 0.14.5 | 每轮循环的 idle 回调限制在约 10ms |
| `khronos-egl`（Linux） | 6.0.0 | 失败的 EGL 调用报告成功时不再 panic |
| `wgpu`、`naga`（Linux） | 30.0.2 | 更新的 API（`QueueDescriptor`、`has_srgb_suffix`） |
| `zed-font-kit`、`zed-scap`、`zed-xim` | crates.io 发布版本 | 之后的小修复 |
| `oo7`（Linux） | 0.6 | 提示流结束时的错误处理 |
| `windows-capture`（Windows，`screen-capture`） | 1.4.4，在 `Cargo.lock` 中固定 | 无；1.5 无法与 `zed-scap` 一起构建，所以开启 `screen-capture` 的一方也必须固定在 1.4.4 |

## 相对原始代码改动的文件

所有改动过的文件（包括 crate 清单）开头都有一行 `Modified for Agentty` 注释。

- `crates/gpui/src/style.rs`：测试使用 `#[test]` 而不是 `#[perf]`。
- `crates/gpui_apple/build.rs`：没有 `../gpui` 时（已发布的 crate）读取随附的 `gpui-src/` 副本；脚本让这个副本与 `crates/gpui/src` 保持一致。
- `crates/gpui_wgpu/src/{wgpu_context,wgpu_renderer,wgpu_atlas}.rs`：适配 wgpu 30.0.2 的 API。

crate 清单由 `scripts/apply_agentty.py` 生成：包名改为 `agentty-gpui-*`，写入工作区版本，去掉 iOS 和 Web，并把 Git 依赖换成 crates.io。脚本的输出没有提交时，CI 会失败。

## 更新方式

有两个分支：

- **`main`**：发布的代码，为 Agentty 做的改动都在这里。
- **`upstream`**：作为基础的原始代码，未经修改，每个原始版本一个提交，对应的版本写在 `zed/SNAPSHOT.md` 中。

引入新的原始版本：

```sh
scripts/sync-upstream.sh v1.29.0   # 在 upstream 上生成新快照
git merge upstream                 # 合并到 main，只有这里改过的文件可能冲突
python3 -I scripts/apply_agentty.py
```

合并时如果 crate 清单冲突，采用原始一侧（`git checkout --theirs <文件>`），再重新运行脚本。

## 测试

```sh
cargo nextest run --workspace --features agentty-gpui-apple/runtime_shaders
cargo test --workspace --doc --features agentty-gpui-apple/runtime_shaders
```

- 使用 nextest：有几个测试读取进程级状态，用 `cargo test` 并行运行时会失败。
- CI 使用 `--profile ci`（`.config/nextest.toml`），失败的测试最多重试两次，因为有几个基准测试会计时 80ms 的轮询，在繁忙的运行器上可能超时。
- Linux CI 在 wgpu 渲染器测试中使用 Mesa 的软件 Vulkan 驱动。

## 发布流程

1. 提高 `scripts/apply_agentty.py` 中的 `VERSION`，重新运行脚本并推送。CI 必须通过，包括 `cargo publish --workspace --dry-run`。
2. 在 GitHub Actions 中运行 **Agentty** 工作流，用这个检出在 macOS、Windows 和 Linux 上构建并测试 Agentty。
3. 用 `cargo publish --workspace` 按依赖顺序发布。
4. 打上发布标签（`git tag v0.1.0 && git push origin v0.1.0`）。

## 反馈问题

agentty-gpui 的问题或希望的改动，请提交到 [agentty-gpui 的 Issues](https://github.com/empty-user77/agentty-gpui/issues)；Agentty 的问题请提交到 [Agentty 的 Issues](https://github.com/empty-user77/Agentty/issues)。各操作系统上 Agentty 的差异见 [平台](/docs/platforms)。
