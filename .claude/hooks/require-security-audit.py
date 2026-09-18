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


def reviewed(what):
    return subprocess.run([sys.executable, AUDIT, "--reviewed", what], capture_output=True, text=True).returncode == 0


CD = re.compile(r"(?:^|[;&|(]\s*|\s)cd\s+([^\s;&|)]+)")
DASH_C = re.compile(r"\s-C\s+([^\s;&|)]+)")
REDIRECTED = re.compile(r"--git-dir|--work-tree|\bGIT_DIR\b|\bGIT_WORK_TREE\b|\bpushd\b|\bpopd\b|\bchdir\b")


def elsewhere(unquoted, invocation, before, cwd):
    """Whether a git command runs in another repository (a scratch repository of a test, say).

    Read off the command only: `git -C <dir>`, else the `cd <dir>` steps before it, starting from the
    tool's working directory. Whatever cannot be read — a variable, a substitution, a quoted path,
    `--git-dir`, `GIT_DIR`, `pushd` — counts as this repository, and so does a `cd` that may not hold
    when git runs: one inside a subshell that has closed, or into a folder that is not there (the
    `cd` fails and the command goes on here). The gate fails closed.
    """
    if REDIRECTED.search(unquoted):
        return False
    steps = list(CD.finditer(before))
    if steps and ")" in before[steps[-1].end() :]:
        return False
    root = os.path.realpath(ROOT)

    def plain(path):
        return path and not re.search(r"""[$`"'*?\\]""", path) and path != "-"

    explicit = DASH_C.search(invocation)
    if explicit:
        path = explicit.group(1)
        if not plain(path):
            return False
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
            return False
    if not os.path.isdir(current):
        return False
    current = os.path.realpath(current)
    return not (current == root or current.startswith(root + os.sep))


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
    if commit and not elsewhere(unquoted, commit.group(0), unquoted[: commit.start()], cwd):
        if STAGING.search(unquoted[: commit.start()]):
            block("this command stages and commits in one go, so the committed tree cannot be the reviewed one.")
        if COMMIT_ALL.search(commit.group(1)):
            block("`git commit -a` (or --include/--only) commits files that were not staged when the change was reviewed.")
        if not reviewed("commit"):
            block("the staged tree has not been through the security audit.")
    push = PUSH.search(unquoted)
    if push and not elsewhere(unquoted, push.group(0), unquoted[: push.start()], cwd) and not reviewed("push"):
        block("the commit being pushed (HEAD) has not been through the security audit.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
