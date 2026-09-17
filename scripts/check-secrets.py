#!/usr/bin/env python3
"""Secret scanner used by git hooks, Claude Code hooks and CI.

Blocks real credentials (API keys, tokens, private keys) from entering the repository.
Test fixtures must use obviously fake values (see ALLOW_MARKERS).

Usage:
  check-secrets.py --staged              scan lines added in the staged diff (pre-commit)
  check-secrets.py --range A..B          scan lines added in a commit range (pre-push)
  check-secrets.py --all                 scan every tracked file (CI)
  check-secrets.py --stdin [--label X]   scan text from stdin (Claude Code hooks)
  check-secrets.py FILE...               scan files

Exit status: 0 clean, 1 secrets found, 2 usage error.
Findings are printed masked; the secret itself is never echoed.
"""

import re
import subprocess
import sys

PATTERNS = [
    ("Figma token", r"figd_[A-Za-z0-9_-]{20,}"),
    ("GitHub token", r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{30,}"),
    ("GitHub fine-grained token", r"github_pat_[A-Za-z0-9_]{30,}"),
    ("Anthropic API key", r"sk-ant-[A-Za-z0-9_-]{20,}"),
    ("OpenAI API key", r"\bsk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_-]{32,}"),
    ("Slack token", r"\bxox[abposr]-[A-Za-z0-9-]{10,}"),
    ("Slack webhook", r"hooks\.slack\.com/services/T[A-Za-z0-9]+/B[A-Za-z0-9]+/[A-Za-z0-9]+"),
    ("AWS access key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
    ("Google API key", r"\bAIza[0-9A-Za-z_-]{35}\b"),
    ("Stripe key", r"\b(?:sk|rk)_live_[A-Za-z0-9]{20,}"),
    ("npm token", r"\bnpm_[A-Za-z0-9]{36}\b"),
    ("Notion token", r"\b(?:secret|ntn)_[A-Za-z0-9]{40,}"),
    ("Linear API key", r"\blin_api_[A-Za-z0-9]{30,}"),
    ("Sentry token", r"\bsntrys_[A-Za-z0-9_=+/-]{30,}"),
    ("JWT", r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"),
    ("Private key", r"-----BEGIN (?:[A-Z]+ )*PRIVATE KEY-----"),
    ("Apple app-specific password", r"\b[a-z]{4}-[a-z]{4}-[a-z]{4}-[a-z]{4}\b(?=.*(?:PASSWORD|password|APPLE))"),
    (
        "Credential assignment",
        r"""(?i)\b(?:api[_-]?key|secret|token|password|passwd|access[_-]?key|auth)\w*["']?\s*[:=]\s*["'][A-Za-z0-9_\-+/=.]{24,}["']""",
    ),
]
COMPILED = [(name, re.compile(rx)) for name, rx in PATTERNS]

# A line is allowed when the matched value is clearly fake.
ALLOW_MARKERS = re.compile(r"(?i)example|not[_-]?a[_-]?real|fake|dummy|placeholder|sample|redacted|xxxx|\*\*\*|<[a-z_ -]+>|your[_-]")

# Files that document the patterns themselves.
SKIP_PATHS = re.compile(r"(^|/)(scripts/check-secrets\.py|\.gitleaks\.toml)$")


def mask(value):
    return value[:6] + "…" + f"({len(value)} chars)" if len(value) > 8 else "…"


def scan_lines(lines, label):
    findings = []
    for number, line in lines:
        for name, rx in COMPILED:
            for match in rx.finditer(line):
                if ALLOW_MARKERS.search(match.group(0)):
                    continue
                findings.append((label, number, name, mask(match.group(0))))
    return findings


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True, check=False).stdout


def added_lines(diff):
    """(path, line number, text) for every added line of a unified diff."""
    path, number = None, 0
    for line in diff.splitlines():
        if line.startswith("+++ "):
            path = line[6:] if line.startswith("+++ b/") else None
        elif line.startswith("@@"):
            m = re.search(r"\+(\d+)", line)
            number = int(m.group(1)) - 1 if m else 0
        elif line.startswith("+") and not line.startswith("+++"):
            number += 1
            if path and not SKIP_PATHS.search(path):
                yield path, number, line[1:]
        elif not line.startswith("-"):
            number += 1


def scan_diff(diff):
    findings = []
    for path, number, text in added_lines(diff):
        findings += scan_lines([(number, text)], path)
    return findings


def scan_file(path):
    if SKIP_PATHS.search(path):
        return []
    try:
        with open(path, encoding="utf-8", errors="ignore") as handle:
            return scan_lines(enumerate(handle, 1), path)
    except (IsADirectoryError, FileNotFoundError):
        return []


def main(argv):
    if not argv:
        print(__doc__, file=sys.stderr)
        return 2
    if argv[0] == "--staged":
        findings = scan_diff(git("diff", "--cached", "-U0", "--no-color"))
    elif argv[0] == "--range" and len(argv) > 1:
        findings = scan_diff(git("diff", "-U0", "--no-color", argv[1]))
    elif argv[0] == "--all":
        findings = []
        for path in git("ls-files").splitlines():
            findings += scan_file(path)
    elif argv[0] == "--stdin":
        label = argv[argv.index("--label") + 1] if "--label" in argv else "<input>"
        findings = scan_lines(enumerate(sys.stdin.read().splitlines(), 1), label)
    else:
        findings = []
        for path in argv:
            findings += scan_file(path)

    if not findings:
        return 0
    print("✋ Possible secrets found (values masked):", file=sys.stderr)
    for label, number, name, masked in findings:
        print(f"  {label}:{number}  {name}  {masked}", file=sys.stderr)
    print(
        "Use an obviously fake value in code, tests and docs (e.g. figd_example_not_a_real_key).\n"
        "Never copy real credentials from local config (~/.claude.json, .env, Keychain) into the repository.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
