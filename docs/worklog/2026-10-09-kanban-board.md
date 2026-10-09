# Development board (kanban) — 2026-10-09

Branch `feat/kanban-board` (worktree `../Agentty-kanban`, from `origin/main`).

## What it is

A board page (activity bar icon `square-kanban`, palette, `page board` deep link) with eight stages:
backlog → instructed → in development → developed → code review → QA → more instructions → done.
Off by default: **Settings → Development board** turns it on.

- Tickets: `~/.agentty/board.json` (`0600`, read again before every change since all windows share it).
  A ticket links to its workspace by `(slot, workspace id)`.
- Moving a ticket to *instructed* (drag, ▶ on the card, "Save and instruct"): a worktree of the ticket's
  project (or the project folder, per settings), a workspace in the **Board** group (found by name in
  any language, like the plugins group), an agent started with `ticket_prompt`. The user stays on the board.
- Automatic stages (`next_stage`): *in development* when any agent of the workspace is in a turn;
  *developed* only on a finished turn / exited pane, when nothing is busy. Busy = in turn, needs the
  user, a pane started for the ticket that has not finished its first turn (`BoardState::pending`), or
  task reports not yet delivered to the lead.
- Splitting: a ticket pane's `agentty tasks` request → Auto (start at once, tasks branch from the
  ticket's branch via `worktree::create_from`), Ask (usual dialog, still from the ticket's branch), Off
  (refused, and the prompt tells the agent not to split). When every task has finished and the lead
  (first pane of the first tab) is free, the lead is told which ones to merge (`tasks_done_message`).
- *More instructions*: dialog → typed into the lead when free, else `deliver_prompt` with
  `PromptTarget::Workspace` (wakes a dormant workspace). A ticket whose workspace is gone starts over
  with the instructions appended.
- Review / QA / done are the user's; "Sign off as done" in the ticket dialog.
- Clean-up at sign-off (setting: Ask (default) / Always / Never): the ticket's linked worktrees (its
  own and those of task panes still open) are checked with `worktree::cleanup_check`; the dialog shows
  per tree what goes. Clean up = `close_workspace` + `worktree::remove_linked(tree, tree, false, false)`
  for trees without uncommitted changes: unmerged branches stay, merged `agentty/…` branches go.
  Always cleans up unasked only when nothing would be lost (no changes, no non-build ignored files).
- Jira Cloud (`agentty-bridge/src/jira.rs`): site / e-mail / JQL in settings, API token in the
  Keychain (`run.agentty.jira`, scoped per data folder). "Import from Jira" on the board adds backlog
  tickets for issues not yet imported (`jira_key`), titled `KEY: summary`, body = ADF description +
  link. Uses `/rest/api/3/search/jql`, redirects off, https only.

## Verified in the debug app (scratch data folder)

Board render; ▶ → worktree + Board group workspace → instructed → in dev → developed with the agent's
commit; more instructions → in dev → developed with a second commit; a ticket that asked for two
parallel tasks → both started without a dialog from the ticket branch, both committed, lead merged,
one transition to developed. Settings: off hides icon/page, on shows options, Jira fields, empty-site
error. Sign-off of the parallel ticket → dialog listed its tree and both task trees (unmerged
commits) → clean up closed the workspace, removed the three trees and kept the three branches.
Not tried against a real Jira site (no account here).

## Left for later

- PR / CI status on cards; merging the ticket branch after sign-off.
- Writing back to Jira (comment / transition on done).
- Jira token not in the config backup's secret list.
- User docs in `docs/{en,ko,ja,zh}`.
- A pane only starts when the window paints: in a background test window nothing happens until it is shown.
