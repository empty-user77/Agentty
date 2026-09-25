---
title: 平台
description: macOS、Windows 和 Linux 之间的差异。
---

Agentty 在 macOS 上开发，macOS 是基准平台。三个平台都已提供下载，Windows 和 Linux 具备所有不依赖 AppKit 或 WebKit 的功能。

## 差异

| | macOS | Windows | Linux |
|---|---|---|---|
| 窗格 shell | `$SHELL` | PowerShell（装了就用 `pwsh`） | `$SHELL` |
| 凭据存储 | 钥匙串 | 凭据管理器 | Secret Service，没有则用私有文件 |
| 通知 | 系统通知，点击打开窗格 | Toast | `notify-send` |
| 内置浏览器 | 在 Agentty 内 | 默认浏览器 | 默认浏览器 |
| 菜单栏图标、迷你模式 | ✓ | — | — |
| 标题栏 | Agentty 自绘 | 系统 | 系统；合成器无装饰时由 Agentty 自绘 |
| 安装 | DMG | 安装程序（按用户） | `.deb` / `.rpm` |
| 自动更新 | 安装后重启 | 安装后重启 | 指向新的软件包页面 |

快捷键会转换：⌘ → Ctrl+Shift、⇧⌘ → Ctrl+Alt+Shift、⌥⌘ → Ctrl+Alt、⌘1…9 → Alt+1…9。参见[键盘快捷键](/docs/keyboard-shortcuts)。

## Windows

需要 Windows 10 版本 1809 及以上，x64。安装程序以当前用户身份安装，无需管理员权限，并添加开始菜单项、`agentty://` 链接、文件夹上的“在 Agentty 中打开”，以及把 `agentty` 加入 `PATH`。

Claude Code 通过 Git Bash 运行钩子，因此窗格状态需要 **Git for Windows**。**设置 → 系统检查**会列出缺失项并在新标签页中安装。

## Linux

X11 与 Wayland 均支持。软件包会安装 `/usr/bin/agentty`、桌面项（应用菜单与 `agentty://` 链接）和图标，并依赖 GPUI 所需的库。建议安装 Vulkan 驱动。

更新会在应用内提示，由包管理器完成安装。

## 单实例

第二次启动、`agentty://` 链接或“在 Agentty 中打开”都会把任务交给正在运行的 Agentty 然后退出，因此两个进程绝不会共享设置和工作区文件。