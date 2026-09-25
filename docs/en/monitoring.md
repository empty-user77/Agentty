---
title: Monitoring
description: What your AI work costs, which AI processes are running, what your tabs talk to, every git worktree on this computer, and what fills the disk.
---

**⌥⌘U** opens Monitoring. Everything here is computed on your machine; nothing is uploaded.

## AI usage

Cost, calls, cache hit rate and tokens, broken down by model, project and tool. The numbers come from the transcript files the agent CLIs already wrote on your disk.

Counting is not as simple as summing lines. Claude Code writes one JSONL line per content block, so a single API response appears several times with the same message id; those are de-duplicated by that id. Codex writes one usage record per response instead, keyed by response id. Agentty follows each format so a response is counted once.

Claude model prices are built in. For other models — the ones Codex uses, for instance — add prices in `~/.agentty/pricing.json` and they are included:

```json
{
  "some-model": { "input": 1.25, "output": 10.0, "cacheRead": 0.125 }
}
```

Prices are USD per million tokens. `cacheRead`, `cacheWrite` and `cacheWrite1h` are optional and default to 10%, 125% and 200% of the input rate.

> [!NOTE]
> These are estimates for your own orientation, computed from local files. Your provider's invoice is the real number.

## Processes

The AI processes running on your machine right now, with what started them. Useful when something is still burning tokens after you thought it stopped.

## Proxy

A capture proxy that lists what your tabs talk to: start capture, open a tab, filter by endpoint.

- Tabs opened **while capture is on** are routed through a proxy running on your own machine.
- HTTPS stays encrypted. Only the host, port, byte counts and timing are recorded — no certificate is installed and nothing is decrypted.
- For plain HTTP, the method, path and status are recorded, never headers or bodies.
- Records live in memory, are limited in number, are never written to disk, and disappear when Agentty quits.

Stopping capture stops the recording. Tabs that were given the proxy keep using it so they don't lose their network.

## Worktrees

Every git worktree on this computer, grouped by project: its branch, when it was last worked on (its last commit or newest uncommitted change), uncommitted files, commits the default branch doesn't have yet, its pull request (open, merged or closed — read through the GitHub CLI you are signed in to), and its size.

Worktrees are looked for in the folders under your home folder, in Agentty's worktree folder and in the folders you have open. Projects with nothing but their own folder are left out.

Tick several and press **Remove** — or **Select safe ones** first, which picks every tree with no uncommitted changes whose commits are already merged. The dialog asks first:

- **Delete their branches too** removes a branch only when nothing on it would be lost: git's own check, or a pull request merged at exactly the branch's last commit (so squash merges count). Other branches stay.
- Trees with uncommitted changes are skipped unless you tick that they go too — those changes are then lost.
- The project folder itself and a tree a tab is working in can't be selected.

## Disk

How full the disk is — in use, can be cleared, free — and which folders in your home folder take the room, each opening onto its own biggest folders.

What can be cleared is only what is made again by itself:

- **Project build output** — per project, **Remove build junk** (Cargo `target`, `.next`, Gradle `build`, SwiftPM `.build`, …) and **Clear cache** (`.turbo`, `.parcel-cache`, `node_modules/.cache`, pytest/mypy/Ruff caches, …). A folder only counts when its tool's project file sits next to it, and Cargo's `target` also needs the tag Cargo writes into it.
- **Caches** — the development tools' own caches: npm, Yarn, Bun, pip, uv, Go, the Cargo registry, Gradle, Homebrew, CocoaPods, JetBrains, and Xcode DerivedData.
- **Other** — the trash and log files.

Dependencies such as `node_modules` or `~/.m2`, and anything a project needs to run, are never offered. Each removal is confirmed first and checked again right before it happens; links are never followed. The next build or install just takes a little longer.
