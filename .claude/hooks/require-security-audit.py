#!/usr/bin/env python3
"""Claude Code PreToolUse guard: no `git commit` or `git push` before the security-audit skill ran.

The skill (`.claude/skills/security-audit`) ends with `scripts/security-audit.py --mark`, which
records the tree it reviewed. This hook lets a commit through only when the staged tree is that
tree, and a push only when HEAD's tree is — so what is reviewed is what is committed.

Blocks (exit 2, reason sent back to Claude):
- `git commit` / `git push` when the tree was not reviewed.
- `git commit -a` and commands that stage and commit in one go: the tree would not be the reviewed one.
"""

import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
AUDIT = os.path.join(ROOT, "scripts", "security-audit.py")
MARK_FILE = "agentty-security-audit.json"  # in the git dir, as `security-audit.py --mark` writes it

GIT = r"\bgit\b(?:\s+-[^\s]+(?:\s+[^\s-][^\s]*)?)*\s+"
COMMIT = re.compile(GIT + r"commit\b([^|;&\n]*)")
PUSH = re.compile(GIT + r"push\b")
STAGING = re.compile(GIT + r"(?:add|rm|mv|stage|restore\s+--staged|reset|apply\s+--cached|checkout\s+--|stash\s+pop)\b")
COMMIT_ALL = re.compile(r"(?:^|\s)(?:-[a-zA-Z]*a[a-zA-Z]*|--all|--include|--only|-i|-o)(?:\s|$)")
NESTED = re.compile(r"""(?:\b(?:ba|z|da|k)?sh\b[^|;&\n'"]*\s-[a-zA-Z]*c|\beval|\bxargs[^|;&\n'"]*)\s+(['"])(.*?)\1""", re.S)

HOW = (
    "Run the security-audit skill first (Skill tool: security-audit): stage the change in its own command, "
    "review it against the skill's six areas, fix what it finds, then `python3 scripts/security-audit.py --mark` "
    "and commit or push again. Staging anything after the review means reviewing again."
)


def block(reason):
    print(f"BLOCKED by project rule (CLAUDE.md → Enforcement): {reason}\n{HOW}", file=sys.stderr)
    sys.exit(2)


def reviewed(what, checkout):
    if os.path.realpath(checkout) == os.path.realpath(ROOT):
        return subprocess.run([sys.executable, AUDIT, "--reviewed", what], capture_output=True, text=True).returncode == 0
    # A worktree has its own index and marker. Read here rather than by the worktree's copy of
    # `security-audit.py`, which its branch could have changed; same marker as `--mark` writes.
    tree = ask_git(checkout, "write-tree") if what == "commit" else ask_git(checkout, "rev-parse", "HEAD^{tree}")
    marker = os.path.join(ask_git(checkout, "rev-parse", "--path-format=absolute", "--git-dir"), MARK_FILE)
    try:
        with open(marker, encoding="utf-8") as handle:
            return bool(tree) and any(t.get("tree") == tree for t in json.load(handle).get("trees", []))
    except (OSError, ValueError, AttributeError):
        return False


def ask_git(folder, *args):
    return subprocess.run(["git", "-C", folder, *args], capture_output=True, text=True, check=False).stdout.strip()


def gated_checkout(unquoted, invocation, before, cwd):
    """The checkout whose review gates a git command: this one, or one of its worktrees — wherever it
    lives, `.claude/worktrees/` included — or None for another repository.

    A worktree has its own index and its own review marker, so it is gated by its own; asking this
    checkout instead would block every reviewed commit made in a worktree.
    """
    folder = target(unquoted, invocation, before, cwd)
    if folder is None:
        return ROOT
    root = os.path.realpath(ROOT)
    shared = ask_git(ROOT, "rev-parse", "--path-format=absolute", "--git-common-dir")
    if shared and ask_git(folder, "rev-parse", "--path-format=absolute", "--git-common-dir") == shared:
        return ask_git(folder, "rev-parse", "--show-toplevel") or ROOT
    inside = folder == root or folder.startswith(root + os.sep)
    return ROOT if inside else None


CD = re.compile(r"(?:^|[;&|(]\s*|\s)cd\s+([^\s;&|)]+)")
DASH_C = re.compile(r"\s-C\s+([^\s;&|)]+)")
REDIRECTED = re.compile(r"--git-dir|--work-tree|\bGIT_DIR\b|\bGIT_WORK_TREE\b|\bpushd\b|\bpopd\b|\bchdir\b")


def target(unquoted, invocation, before, cwd):
    """The folder a git command runs in (a scratch repository of a test, say), or None when it cannot
    be told.

    Read off the command only: `git -C <dir>`, else the `cd <dir>` steps before it, starting from the
    tool's working directory. Whatever cannot be read — a variable, a substitution, a quoted path,
    `--git-dir`, `GIT_DIR`, `pushd` — counts as this repository, and so does a `cd` that may not hold
    when git runs: one inside a subshell that has closed, or into a folder that is not there (the
    `cd` fails and the command goes on here). The gate fails closed.
    """
    if REDIRECTED.search(unquoted):
        return None
    steps = list(CD.finditer(before))
    if steps and ")" in before[steps[-1].end() :]:
        return None

    def plain(path):
        return path and not re.search(r"""[$`"'*?\\]""", path) and path != "-"

    explicit = DASH_C.search(invocation)
    if explicit:
        path = explicit.group(1)
        if not plain(path):
            return None
        current = os.path.join(cwd, os.path.expanduser(path))
    else:
        current, known = cwd, True
        for step in steps:
            path = step.group(1)
            if not plain(path):
                known = False
            elif os.path.isabs(os.path.expanduser(path)):
                current, known = os.path.expanduser(path), True
            elif known:
                current = os.path.join(current, path)
        if not known:
            return None
    if not os.path.isdir(current):
        return None
    return os.path.realpath(current)


def main():
    try:
        event = json.load(sys.stdin)
    except json.JSONDecodeError:
        return 0
    if event.get("tool_name") != "Bash":
        return 0
    command = (event.get("tool_input") or {}).get("command", "")
    # Words inside quoted strings (commit messages, heredoc prose) are not commands…
    unquoted = re.sub(r"<<-?\s*'?(\w+)'?.*?^\s*\1\s*$", "", command, flags=re.S | re.M)
    # …unless the string is itself run as a command (`sh -c '…'`, `eval "…"`).
    nested = " ; ".join(m.group(2) for m in NESTED.finditer(unquoted))
    unquoted = re.sub(r'"(?:\\.|[^"\\])*"|\'[^\']*\'', '""', unquoted) + " ; " + nested

    cwd = event.get("cwd") or ROOT
    commit = COMMIT.search(unquoted)
    checkout = commit and gated_checkout(unquoted, commit.group(0), unquoted[: commit.start()], cwd)
    if checkout:
        if STAGING.search(unquoted[: commit.start()]):
            block("this command stages and commits in one go, so the committed tree cannot be the reviewed one.")
        if COMMIT_ALL.search(commit.group(1)):
            block("`git commit -a` (or --include/--only) commits files that were not staged when the change was reviewed.")
        if not reviewed("commit", checkout):
            block("the staged tree has not been through the security audit.")
    push = PUSH.search(unquoted)
    checkout = push and gated_checkout(unquoted, push.group(0), unquoted[: push.start()], cwd)
    if checkout and not reviewed("push", checkout):
        block("the commit being pushed (HEAD) has not been through the security audit.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
