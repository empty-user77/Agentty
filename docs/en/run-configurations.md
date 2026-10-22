---
title: Run configurations
description: Start a project's dev server or app with one button, and read its output in a run log beside the terminals.
---

Agentty finds what a project can run, the way an IDE does, and puts a **Run** button in the pane header (in the tab strip when the tab has a single pane). Runs never type into a terminal: their output goes to a run log docked under the terminals.

## What is found

| Project | Run configurations |
|---|---|
| Node (`package.json`) | The scripts of the root package; in packages below it, the ones that start something (`dev`, `start`, `serve`, `preview`, `watch`, `storybook`). The package manager comes from `packageManager` or the lockfile: npm, pnpm, yarn or bun |
| Spring Boot (Gradle or Maven) | Each application, through the wrapper when the project has one; multi-module builds included |
| Anything else | Commands you add yourself with **Add command…** (with an optional name), kept for that project |

Detection only reads files, never runs a build tool. It looks in the project folder and two levels below it (`web/`, `apps/admin/`) and skips dependency and build output folders.

## Running

- The **⌄** beside the button lists the project's configurations, the ones you ran lately first. Picking one only selects it: the button then shows it, and **Run** starts it.
- **Run** starts the configuration. While it runs, the button turns into **Stop** and **Restart**.
- The status bar shows **Run** (or how many are running). Its popover lists the configurations with their state and ports, and every process of the project that listens on a port, each with a button to stop it or open it in the browser. Ports are not shown on Windows.
- If a run fails because its port is taken, the log names the process holding it and offers **Stop it and run again**, but only when that process belongs to the project. Anything else you stop yourself from the open ports, if you are sure.

## The run log

One panel with a tab per run. You can:

- filter the lines by words or a regular expression, with or without matching case
- show only lines of a level and above (with the count for each level)
- wrap long lines, and follow the newest line or scroll back freely
- Cmd-click (Ctrl-click) an address to open it

Colours are kept and progress lines are collapsed into one. Drag the panel's top edge to resize it; the height is kept. Hiding the log keeps the runs going.

## When Agentty quits

Runs end with Agentty: quitting stops them, and runs left behind by a crash are stopped at the next start.
