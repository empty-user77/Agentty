---
title: Git and working trees
description: The git page, branch switching from any pane, and the worktree per session that keeps agents out of each other's way.
---

## The git page

**⇧⌘G** opens a page in the style of GitHub Desktop:

- Switch repository and branch.
- **Fetch** (⇧⌘T), **pull**, **push** (⌘P), **refresh** (⌘R).
- Review the diff file by file, stage what you want, and commit (⌘↩).
- Browse history, and merge branches.

Agentty drives your own `git`, with your configuration and credentials. Every command runs non-interactively, so nothing can hang waiting for a prompt you can't see — if a push needs credentials your credential helper does not have, it fails with a message instead of stalling.

## From a pane

Click the branch name in any pane header to switch branches, pull, push, or copy the name — without leaving your terminal.

Right-click a branch in that list for more: **Switch to this branch**, **Rename…**, **New branch from here…**, **Copy name** and **Delete branch**. A branch with commits that are not merged anywhere is only deleted after a second question, and the dialog can also delete its remote copy (**Also delete … on the server**); **Delete on the server** removes a remote branch for everyone, after asking. Git opens a branch in one folder at a time: when a switch is refused because another folder has that branch, Agentty offers **Open that folder** instead of failing silently.

### Resolve with AI

When a merge stops on conflicts, a pull can't go ahead because the branch and its upstream have diverged, or conflicts are waiting to be resolved, the message offers **Resolve with AI**. It opens Claude Code in the repository — or Codex, or another installed agent if Claude Code is missing — with instructions for the conflict. With no agent installed it opens a plain terminal there. A second merge is not started while one is still under way.

## A worktree per session

When you open a second AI session in a project where another one is already working, Agentty asks where it goes: **Create a new worktree** starts it in its own git worktree on a new branch, taken from the project's default branch, so the two agents never edit the same files; **Open the existing worktree** starts it right next to the other one, for when two sessions on one branch is what you want. The same question comes up for `claude` or `codex` typed into a terminal.

- Worktrees live in `~/.agentty/worktrees/`, on branches named `agentty/<name>`.
- Turn this off in **Settings → General** if you want every session in the same folder.

### How it works

Agentty runs `git worktree add` on a new branch `agentty/<name>`, taken from the project's **default branch** rather than whatever the project folder currently has checked out — so a session never builds on another session's unmerged work.

The trees are created under `~/.agentty/worktrees/<project>-<hash of its path>/<name>/`, outside the project. Keeping them out of the project folder matters: a copy of the code inside it would be walked by every search, file watcher and build.

Agentty only ever removes trees under that folder. Worktrees you made yourself are listed in the files panel and left alone.

## The files panel

**⌥⌘B** docks the project tree beside your terminals. It also lists **every working tree** of the project, who is working in each one, and what changed there.

Click a tree to see its files — and, when a session is already working in it, to go to that terminal.

Right-click one for the rest: go to the terminal working there, open a new terminal, show the folder in the file manager, copy its path, or remove the tree. A tree whose folder you deleted by hand can be cleaned up from the same menu.

Removing asks first. The dialog names the branch and says what goes with it: the working tree and its folder, and the local branch too when you picked **Remove working tree and branch**. If that branch was pushed, the dialog offers to delete it on the remote as well — the tick is off by default, and the remote branch only goes once the local one really went. Git decides what is safe either way: a tree with uncommitted changes stays, and so does a branch with commits nothing else has.

A tree a tab is still working in is never removed: the menu leaves those entries out, and removing it any other way says so rather than asking. Close that tab first.

### Working trees in the panel

The **Working trees** section lists the worktrees of the repository the pane is in. In a folder of projects, such as your home folder, it lists every project under it that has worktrees: grouped by project, each group foldable, the project you worked on last first — and inside it the project folder, then its worktrees, newest first. Each row says when the tree was last worked on (its last commit or newest uncommitted change). The folders are only scanned while the panel is open, and the section can be resized by dragging. The refresh button scans again.

When there are worktrees, the **Files** button turns purple and shows their count.

Double-click a worktree, or right-click it and choose **Open in this terminal**, to carry on there with your shell (Agentty asks first). **Clean up and go back to the project folder** removes the tree and moves the terminal back to the project: at once when the branch is merged and nothing is uncommitted, otherwise a dialog says exactly what would be lost. It is refused while an agent, a busy shell or another pane works in that tree.

A shell whose worktree folder was removed under it lands in the project folder instead of a folder that no longer exists.

### What removing keeps

A branch goes with its worktree only when the default branch already holds all its commits. A branch with commits that are not in the default branch stays — also when it was pushed — and the same goes for the **Worktrees** page under [Monitoring](/docs/monitoring). Ignored build folders (`build/`, `out/`, `dist/`, `.cache/`, `coverage/`) are judged by what is in them: one that holds files is asked about, while `target/`, `node_modules/` and the like are removed as before.

## Parallel tasks

Ask an agent to split work up — "do A, B and C in parallel". It asks Agentty, you confirm once, and each task starts in its own split pane and its own worktree.

You confirm every such request; an agent cannot start panes on its own.

## The file editor

Click a file in the files panel to open it next to your terminals: syntax colors for the common languages, brackets and quotes typed in pairs (the matching bracket boxed), **⌘/** to comment lines out, **⌥↑** / **⌥↓** to move them, undo and redo, **⌘S** to save, **⇧⌥F** to format with the formatter you have installed (Prettier, google-java-format, ktlint, rustfmt, gofmt, Black) or, without it, the language server.

Pictures (PNG, JPEG, GIF, WebP) open as a preview, scaled down to fit. Files an agent changes reload by themselves. **Open in editor** hands the file to VS Code, Cursor or your system editor. Files are only ever read and written, never run.

### Finding your way around the code

For Kotlin, Java, JavaScript, TypeScript, Python, HTML, CSS (Tailwind included), Rust and Go:

| | |
|---|---|
| **⌘P** | Go to a file by name. Type `@` for the symbols of this file, `#` for the symbols of the project, `:` for a line |
| **F12** or **⌘-click** | Go to where the name under the cursor is defined (hold **⌘** over a name to see its definition without leaving). On an import's path, open that file. On a definition itself, list its usages |
| **⇧F12** or **⌥F7** | Every place the name is used in the project |
| **F8** / **⇧F8** | Next / previous problem in the file, with its message |
| **⌃Space** | Completions (they also open as you type a name or after `.`; ↩ or ⇥ puts one in, Esc closes) |
| **F2** | Rename the name under the cursor everywhere it is used (with a language server; in this file only without one) |
| **⌘.** | Quick fixes and refactorings the language server offers here (a light bulb in the gutter says the line has some) |
| **⇧⌘O** / **⌥⌘O** / **⌃G** | Symbols of this file / of the project / a line |
| **⌃-** / **⌃⇧-** | Back and forward through the places you jumped to |

Definitions are found by name in the project's files (`.gitignore` is respected; dependency and build folders are left out), so nothing has to be installed. When a name is defined in more than one place, the list shows them all, closest first.

When the language's server is installed, the editor asks it instead and goes straight to the right definition, lists only the real usages, and shows the type or signature when you hold **⌘** over a name. It starts by itself when you open a file of that language:

| Language | Server (on your `PATH`) |
|---|---|
| Rust | `rust-analyzer` (`rustup component add rust-analyzer`) |
| Go | `gopls` |
| TypeScript, JavaScript | the project's or your TypeScript 7 (`tsc`), or `typescript-language-server` |
| Python | `pyright` (`pyright-langserver`), `basedpyright` or `pylsp` |
| Java | `jdtls` |
| Kotlin | `kotlin-language-server` |

While a server is still reading a large project, or when it finds nothing, the editor answers by name as above. rust-analyzer runs without build scripts and proc macros, so opening a project never runs its code; No server is started in a way that runs the project's own code: the project's TypeScript is used only when your package manager installed it (git leaves `node_modules` untracked), jdtls starts with its Gradle import off, and kotlin-language-server is not started in Gradle or Maven projects, since it runs their build to read them.

The server also reports problems as you type: errors are underlined with a red wavy line, warnings yellow and notes blue, the line number and a tick on the scrollbar take the same color, and resting the pointer on one shows its message. Errors and warnings also write their message at the end of the line (**Settings → Project → File editor** turns that off). The status bar counts the errors and warnings of the file, and **F8** goes to the next one. rust-analyzer reports what it finds without building the project (`cargo check` stays off). When the server has a fix for a problem (such as **Add import from "./bad"**), a light bulb shows in the gutter of the cursor's line and the fix is listed under the message; **⌘.** lists every fix and refactoring for the cursor or the selection. A fix changes the open files as one step that **⌘Z** takes back, and saves the changes to files that are not open. **Fix with AI**, under every problem's message and last in the **⌘.** list, hands the problem — where it is and what the server said — to the connected session. As you type, the server's completions show under the cursor, and choosing a name from another file adds its import; without a server the words of the file are offered. While you type a call, its signature shows above the line with the parameter you are on highlighted. Resting the pointer on a name shows its type or signature; the status bar's error count lists every problem in the open files, with the code beside each.

The status bar at the bottom of the editor names the server of the file in front. While it is not installed it says **Install …**: a click runs the server's official install command in a new terminal tab (`rustup`, `go install`, `npm` or Homebrew), or opens its page when this computer lacks the tool that command needs. **Settings → Project → Language servers** lists them all. A server installed while Agentty runs takes over by itself. Language servers run on this computer; no code is sent anywhere.

### Working with an agent beside the code

**Connect session** in the editor's bar puts an agent's terminal at the right of the code (drag its edge to resize). By default it is the agent of the active tab; pick another from the column. When the session waits for you (a permission or a question), its chip in the bar says so; a click shows it.

- **Select code** and a small bar offers **Ask AI**, **Copy**, **Definition** and **Usages**. **Ask AI** (or **⌘K**) takes an instruction and sends it to the linked session together with the file and the selected lines.
- **See the agent work.** When an agent changes a file, the changed lines light up and fade, with the agent's name (`Claude-3f9a`) at its caret. Claude Code shows the spot just before it writes. Several sessions can work at once: each gets its own caret and a chip in the bar, and clicking a chip opens that file at that spot.
- **Follow** (the eye in the bar) brings each file the agent is about to change to the front, at the spot, opening it if need be. It never takes the keyboard, waits while you type, and does nothing while the terminals are in front. An agent working above or below what you see is pinned to that edge; click it to scroll there.
- **Ask AI** marks the lines it was about until the agent changes them. **⌥↩** sends the instruction as a question only (the agent is told not to change files).
- **Review what agents changed.** Every change an agent makes to an open file keeps a bar in the gutter, in the agent's color, and a dot on the file's tab. The bar at the bottom steps through the changes: **Undo** puts those lines back as they were before the agent touched them and saves the file (⌘Z takes it back), **Keep** clears the mark, **Keep all** clears the file. **Compare** shows the lines as they were above the lines as they are; **N changes** in the bar lists every change in every open file, with the code beside each; **⌥F5** / **⇧⌥F5** step to the next and previous change. Edits the agent makes on the same lines again fold into one change.
- **Your edits are safe.** When an agent writes a file you have unsaved edits in, the two are merged if they touch different lines; only edits on the same lines ask what to do. Your cursor and view stay on the code you were reading when the agent adds lines above it.
- The bar above the code shows where the cursor is (`ApiClient › save`); click a part to go to it. In a list of definitions or usages, the code around the selected place shows beside it.
