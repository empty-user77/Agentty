---
title: 键盘快捷键
description: 全部默认快捷键，以及它们在 Windows 与 Linux 上的对应。
---

完整列表也在**设置 → 键盘快捷键**中，按你所在平台的表示法显示。

## Windows 与 Linux

快捷键以 macOS 为准定义再做转换，好让 Ctrl+字母始终交给 shell：

| macOS | Windows / Linux |
|---|---|
| ⌘X | Ctrl+Shift+X |
| ⇧⌘X | Ctrl+Alt+Shift+X |
| ⌥⌘X | Ctrl+Alt+X |
| ⌘1…⌘9 | Alt+1…Alt+9 |
| ⌘= / ⌘- / ⌘0 | Ctrl+= / Ctrl+- / Ctrl+0 |
| ⌘-点击 | Ctrl-点击 |

文本输入框和文件编辑器使用普通的 Ctrl（Ctrl+C / V / X / A / Z、Ctrl+S、Ctrl+Y、按词移动的 Ctrl+←/→、Ctrl+Home / End）。

## 标签页与窗格

| 操作 | 快捷键 |
|---|---|
| 新建终端 / Claude Code / Codex 标签页 | ⌘T / ⌥⌘C / ⌥⌘X |
| 新建工作区 | ⌘N |
| 向右 / 向下分屏 | ⌘D / ⇧⌘D |
| 下一个 / 上一个窗格 | ⌘] / ⌘[ |
| 下一个 / 上一个标签页 | ⇧⌘] / ⇧⌘[、⌃Tab |
| 下一个 / 上一个工作区 | ⌥⌘↓ / ⌥⌘↑ |
| 关闭窗格 / 标签页 | ⌘W / ⇧⌘W |
| 跳到工作区 / 标签页 1–9 | ⌘1…⌘9 / ⌃1…⌃9 |

## 面板与页面

| 操作 | 快捷键 |
|---|---|
| 切换侧边栏 | ⌘B |
| 工作区 / 本地会话 | ⇧⌘E / ⇧⌘S |
| Git / 扩展 | ⇧⌘G / ⇧⌘X |
| Session Flow / 监控 / 设置 | ⇧⌘F / ⌥⌘U / ⌘, |
| 内置浏览器 / 文件面板 | ⇧⌘B / ⌥⌘B |
| 命令面板 / 跳到未读 | ⇧⌘P / ⇧⌘U |
| 搜索本地会话 | ⇧⌘O |
| 迷你模式 | ⌃⌘M |

## 终端与编辑器

| 操作 | 快捷键 |
|---|---|
| 在回滚缓冲中查找 | ⌘F |
| 字体放大 / 缩小 / 重置 | ⌘= / ⌘- / ⌘0 |
| 复制 / 粘贴 / 全选 / 清屏 | ⌘C / ⌘V / ⌘A / ⌘K |
| Git 页面：提交 / push / fetch / 刷新 | ⌘↩ / ⌘P / ⇧⌘T / ⌘R |
| 文件编辑器：保存 / 格式化 / 关闭文件 | ⌘S / ⇧⌥F / ⌘W |
| 文件编辑器：切换 Markdown 预览（macOS） | ⇧⌘V |

双击标题栏可缩放窗口；双击标签栏空白处可新建标签页。

## 你自己的命令

命令面板（**⇧⌘P**）也会列出你在项目 `agentty.json` 或 `~/.agentty/commands.json` 中定义的条目。保留词同样有效 —— 在终端里输入 `claude yolo` 会展开为 `claude --dangerously-skip-permissions`。
