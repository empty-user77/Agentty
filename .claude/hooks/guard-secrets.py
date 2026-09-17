#!/usr/bin/env python3
"""Claude Code PreToolUse guard: secrets never reach files or commits, and the guards stay on.

Blocks (exit 2, reason sent back to Claude):
- Write / Edit / MultiEdit / NotebookEdit whose new content contains a credential.
- Bash commands that contain a literal credential.
- Bash commands that bypass or disable the git secret hooks (--no-verify, core.hooksPath, …).
"""

import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SCANNER = os.path.join(ROOT, "scripts", "check-secrets.py")

BYPASS = [
    # Scoped to actual git invocations, so prose that mentions the flags (docs, commit messages) still works.
    (r"\bgit\b[^|;&\n]*\s--no-verify\b", "skipping git hooks (--no-verify)"),
    (r"\bgit\s+(?:-[^\s]+\s+)*commit\b[^|;&\n]*\s-[a-zA-Z]*n[a-zA-Z]*\b", "skipping git hooks (git commit -n)"),
    (r"\bgit\b[^|;&\n]*\bconfig\b[^|;&\n]*\bcore\.hooksPath\b\s+(?!\.githooks\b)\S", "pointing core.hooksPath away from .githooks"),
    (r"\bgit\b[^|;&\n]*\bconfig\b[^|;&\n]*--unset[^|;&\n]*\bcore\.hooksPath\b", "unsetting core.hooksPath"),
    (r"\bHUSKY=0\b|\bSKIP_HOOKS?\b", "disabling hooks via environment"),
    (r"(?:rm|mv|chmod\s+-x|>\s*)\s*[^|;&]*\.githooks", "removing or disabling .githooks"),
    (r"(?:rm|mv|>\s*)\s*[^|;&]*(?:check-secrets\.py|guard-secrets\.py)", "removing the secret scanners"),
]


def scan(text, label):
    result = subprocess.run([sys.executable, SCANNER, "--stdin", "--label", label], input=text, capture_output=True, text=True)
    return result.returncode, result.stderr


def block(reason):
    print(reason, file=sys.stderr)
    sys.exit(2)


def main():
    try:
        event = json.load(sys.stdin)
    except json.JSONDecodeError:
        return 0
    tool = event.get("tool_name", "")
    data = event.get("tool_input", {}) or {}

    if tool in ("Write", "Edit", "MultiEdit", "NotebookEdit"):
        path = data.get("file_path") or data.get("notebook_path") or "<file>"
        parts = [data.get("content"), data.get("new_string"), data.get("new_source")]
        parts += [edit.get("new_string") for edit in data.get("edits", []) if isinstance(edit, dict)]
        text = "\n".join(p for p in parts if isinstance(p, str))
        if text:
            code, report = scan(text, os.path.relpath(path, ROOT) if path.startswith(ROOT) else path)
            if code == 1:
                block(
                    "BLOCKED by project rule (CLAUDE.md → Secrets): the content contains what looks like a real credential.\n"
                    + report
                    + "Use an obviously fake placeholder (e.g. figd_example_not_a_real_key) and never copy values from the user's environment."
                )
    elif tool == "Bash":
        command = data.get("command", "")
        # Flags inside quoted strings (commit messages, heredoc prose) aren't flags.
        unquoted = re.sub(r'"(?:\\.|[^"\\])*"|\'[^\']*\'', '""', command)
        for pattern, what in BYPASS:
            if re.search(pattern, unquoted):
                block(f"BLOCKED by project rule (CLAUDE.md → Secrets): {what} is not allowed. Fix the finding instead of bypassing the check.")
        code, report = scan(command, "Bash command")
        if code == 1:
            block("BLOCKED by project rule (CLAUDE.md → Secrets): the command contains what looks like a real credential.\n" + report)
    return 0


if __name__ == "__main__":
    sys.exit(main())
