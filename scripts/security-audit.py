#!/usr/bin/env python3
"""Project security audit: the mechanical half of `.claude/skills/security-audit`.

`check-secrets.py` finds credentials by their shape. This script checks what a pattern scanner
cannot: files that must never be committed whatever they contain, personal data, code that weakens
a guarantee the app makes (agent permissions, the Launch plugin's key handling, release scripts),
and that the guards themselves are still wired in.

Usage:
  security-audit.py --staged          audit what is staged (pre-commit)
  security-audit.py --range A..B      audit a commit range (pre-push)
  security-audit.py --all             audit every tracked file (CI)
  security-audit.py --mark            record that the staged tree passed the AI review (the skill's last step)
  security-audit.py --reviewed commit|push
                                      exit 0 when the tree about to be committed / pushed was reviewed

Exit status: 0 clean (warnings may be printed), 1 blocking findings, 2 usage error, 3 not reviewed.
Values are never echoed: findings name the file, the line and the rule.
"""

import hashlib
import json
import os
import re
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# -- SA01: files that never belong in the repository ------------------------------------------------

FORBIDDEN_FILES = [
    (r"(^|/)\.env(\.(?!example$|sample$)[^/]+)?$", "environment file"),
    (r"(^|/)\.envrc$", "direnv file"),
    (r"\.(pem|p12|pfx|key|keystore|jks|mobileprovision|provisionprofile)$", "key or signing material"),
    (r"(^|/)id_(rsa|dsa|ecdsa|ed25519)(\.pub)?$", "SSH key"),
    (r"(^|/)\.(npmrc|netrc|pypirc)$", "package-manager credentials file"),
    (r"(^|/)(credentials|service[-_]?account[^/]*|client_secret[^/]*)\.json$", "cloud credentials file"),
    (r"(^|/)\.(aws|ssh|gnupg)/", "credentials folder"),
    (r"(^|/)(\.vercel|supabase/\.temp|\.agentty[^/]*)/", "local tool state"),
    (r"(^|/)\.claude/settings\.local\.json$", "personal Claude Code settings"),
    (r"\.(sqlite3?|db)$", "local database"),
    (r"(^|/)\.claude/projects/|\.jsonl$", "conversation transcript"),
    (r"\.(dmg|zip|tar\.gz|tgz)$", "build artifact or archive"),
]

# -- SA02: personal data --------------------------------------------------------------------------

HOME_PATH = re.compile(r"/(?:Users|home)/(?!me\b|you\b|user\b|username\b|name\b|example\b|someone\b|Shared\b|runner\b)[A-Za-z0-9._-]+")
EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[a-z]{2,}")
EMAIL_OK = re.compile(r"(?i)@(([a-z0-9-]+\.)*example\.(com|org|net)|anthropic\.com|users\.noreply\.github\.com|github\.com|[0-9])|^git@")

# -- SA03: code that weakens a guarantee -------------------------------------------------------------
# (rule, regex, severity, paths it applies to, why). Severity "block" fails the audit, "warn" is printed.

CODE_RULES = [
    # Where Agentty itself builds an agent's command line or settings. (The alias feature lets users map
    # their own reserved words to any flag; that is their choice and lives elsewhere.)
    ("agent permissions are never bypassed", r"bypassPermissions|--dangerously-skip-permissions|dangerouslyDisableSandbox", "block", r"^(crates/agentty-app/src/launch\.rs|crates/agentty-bridge/src/idea\.rs|crates/agentty-app/src/workbench/(idea|harness|plugin_host)\.rs|plugins/)", "Agentty must not start an agent without permission checks"),
    ("idea projects get narrow allow rules", r"\"Bash\((curl|wget|sudo|ssh|scp|rm|chmod|git push|open|osascript|security|defaults)\b[^\"]*:\*\)\"", "block", r"^crates/agentty-bridge/src/idea\.rs$", "a broad allow rule runs silently in a non-developer's project"),
    ("service_role keys are never requested", r"--reveal\b", "block", r"^plugins/launch/", "`supabase projects api-keys --reveal` prints secret keys"),
    ("git hooks are not skipped", r"--no-verify\b|config\s+(?:--(?:local|global)\s+)?core\.hooksPath\s+(?!\.githooks\b)[^|&;\s]|--unset\S*\s+core\.hooksPath", "block", r"^(scripts|\.github|plugins|crates)/", "the secret scanners run from these hooks"),
    ("credential scripts do not trace", r"^\s*set\s+-[a-wyz]*x", "block", r"^scripts/.*\.sh$", "`set -x` prints credentials passed to codesign / notarytool"),
    ("workflows do not run untrusted code with secrets", r"pull_request_target", "block", r"^\.github/workflows/", "fork code would run with repository secrets"),
    ("actions are pinned", r"uses:\s*[^@\s]+@(main|master|latest)\b", "warn", r"^\.github/workflows/", "pin third-party actions to a tag or commit"),
    ("secrets are not logged", r"(?i)(eprintln!|println!|console\.(log|error)|plugin\.log)\([^)]*\b(token|secret|password|api_?key)\b", "warn", r"^(crates|plugins|sdk)/", "log the name, never the value"),
    ("network calls use TLS", r"http://(?=[A-Za-z0-9])(?!localhost|127\.0\.0\.1|0\.0\.0\.0|www\.w3\.org|[^/\s\"']*\.example\b)", "warn", r"^(crates|plugins|sdk)/.*\.(rs|mjs|js)$", "plain http leaks what it carries"),
    ("files with user data are private", r"0o(777|666|775|664)\b|chmod\s+(777|666)", "warn", r"^(crates|plugins|scripts)/", "conversation and credential files are 0600, folders 0700"),
    ("prompts and rules are English", r"[\uac00-\ud7a3]", "block", r"^(\.claude/skills/|docs/plugins/|CLAUDE\.md$|crates/agentty-bridge/src/(idea|handoff|harness)\.rs$)", "prompts, rules and skills are written in English only (CLAUDE.md)"),
]
# Where a forbidden thing may be mentioned: the rule tables themselves, docs, tests, comments.
RULE_EXEMPT_PATH = re.compile(r"^(scripts/security-audit\.py|scripts/check-secrets\.py|\.claude/hooks/|\.claude/skills/security-audit/|CLAUDE\.md|docs/)")
RULE_EXEMPT_LINE = re.compile(r"\bassert|audit:\s*ok\b|^\s*(//|#|\*|/\*)")
TEST_PATH = re.compile(r"(^|/)(tests?|__tests__|fixtures)/|\.test\.[a-z]+$|_test\.rs$")

# -- SA04: the guards are still wired in ------------------------------------------------------------

WIRING = [
    (".githooks/pre-commit", ["check-secrets.py", "security-audit.py"]),
    (".githooks/pre-push", ["check-secrets.py", "security-audit.py"]),
    (".claude/settings.json", ["guard-secrets.py", "require-security-audit.py"]),
    (".github/workflows/ci.yml", ["check-secrets.py", "security-audit.py"]),
    (".gitignore", [".env"]),
]

MARK_FILE = "agentty-security-audit.json"


def git(*args):
    return subprocess.run(["git", "-C", ROOT, *args], capture_output=True, text=True, check=False).stdout


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
            if path:
                yield path, number, line[1:]
        elif not line.startswith("-"):
            number += 1


def is_binary(path):
    try:
        with open(os.path.join(ROOT, path), "rb") as handle:
            return b"\0" in handle.read(8192)
    except (IsADirectoryError, FileNotFoundError):
        return True


def all_lines(paths):
    for path in paths:
        if is_binary(path):
            continue
        with open(os.path.join(ROOT, path), encoding="utf-8", errors="ignore") as handle:
            for number, text in enumerate(handle, 1):
                yield path, number, text.rstrip("\n")


def test_section_start(path, cache={}):
    """Line where a Rust file's `#[cfg(test)]` module begins (tests sit at the end of the file)."""
    if path not in cache:
        cache[path] = None
        if path.endswith(".rs"):
            try:
                with open(os.path.join(ROOT, path), encoding="utf-8", errors="ignore") as handle:
                    cache[path] = next((n for n, text in enumerate(handle, 1) if text.strip() == "#[cfg(test)]"), None)
            except FileNotFoundError:
                pass
    return cache[path]


def in_tests(path, number):
    start = test_section_start(path)
    return bool(TEST_PATH.search(path)) or (start is not None and number >= start)


def audit(paths, lines, whole_tree):
    """`paths`: files added or changed. `lines`: (path, number, text) to inspect."""
    findings = []  # (severity, location, rule, why)
    for path in paths:
        for pattern, what in FORBIDDEN_FILES:
            if re.search(pattern, path):
                findings.append(("block", path, f"SA01 {what} must not be committed", "keep it outside the repository; .gitignore it"))
                break
    compiled = [(name, re.compile(rx), severity, re.compile(scope), why) for name, rx, severity, scope, why in CODE_RULES]
    for path, number, text in lines:
        where = f"{path}:{number}"
        if not RULE_EXEMPT_PATH.search(path):
            if HOME_PATH.search(text):
                findings.append(("block", where, "SA02 a real home folder path", "use /Users/me or ~ in code, tests and docs"))
            for address in EMAIL.findall(text):
                if not EMAIL_OK.search(address):
                    findings.append(("warn", where, "SA02 an email address", "fine for a public contact; never a customer's or a teammate's"))
        if RULE_EXEMPT_PATH.search(path) or RULE_EXEMPT_LINE.search(text) or in_tests(path, number):
            continue
        for name, rx, severity, scope, why in compiled:
            if scope.search(path) and rx.search(text):
                findings.append((severity, where, f"SA03 {name}", why))
    if whole_tree:
        for path, needles in WIRING:
            try:
                with open(os.path.join(ROOT, path), encoding="utf-8") as handle:
                    content = handle.read()
            except FileNotFoundError:
                findings.append(("block", path, "SA04 a guard file is missing", "restore it"))
                continue
            for needle in needles:
                if needle not in content:
                    findings.append(("block", path, f"SA04 no longer runs or covers `{needle}`", "the guards must stay wired in"))
    return findings


def report(findings):
    blocking = [f for f in findings if f[0] == "block"]
    warnings = [f for f in findings if f[0] == "warn"]
    if warnings:
        print("security-audit: warnings (not blocking)", file=sys.stderr)
        for _, where, rule, why in warnings:
            print(f"  {where}  {rule} — {why}", file=sys.stderr)
    if blocking:
        print("✋ security-audit: blocked", file=sys.stderr)
        for _, where, rule, why in blocking:
            print(f"  {where}  {rule} — {why}", file=sys.stderr)
        print("A false positive on a single line can be acknowledged with a trailing `audit: ok` comment and a reason.", file=sys.stderr)
        return 1
    return 0


# -- the AI review marker ---------------------------------------------------------------------------


def mark_path():
    git_dir = git("rev-parse", "--git-dir").strip() or ".git"
    return os.path.join(git_dir if os.path.isabs(git_dir) else os.path.join(ROOT, git_dir), MARK_FILE)


def reviewed_trees():
    try:
        with open(mark_path(), encoding="utf-8") as handle:
            return json.load(handle).get("trees", [])
    except (FileNotFoundError, json.JSONDecodeError):
        return []


def staged_tree():
    return git("write-tree").strip()


def mark():
    tree = staged_tree()
    if not tree:
        print("security-audit: nothing to mark (is the index in a merge conflict?)", file=sys.stderr)
        return 2
    trees = [t for t in reviewed_trees() if t.get("tree") != tree]
    trees.append({"tree": tree, "at": int(time.time()), "diff": hashlib.sha256(git("diff", "--cached").encode()).hexdigest()[:16]})
    with open(mark_path(), "w", encoding="utf-8") as handle:
        json.dump({"trees": trees[-100:]}, handle, indent=1)
    print(f"security-audit: reviewed tree {tree[:12]} recorded")
    return 0


def reviewed(what):
    tree = staged_tree() if what == "commit" else git("rev-parse", "HEAD^{tree}").strip()
    if tree and any(t.get("tree") == tree for t in reviewed_trees()):
        return 0
    return 3


def main(argv):
    if not argv:
        print(__doc__, file=sys.stderr)
        return 2
    if argv[0] == "--mark":
        return mark()
    if argv[0] == "--reviewed" and len(argv) > 1 and argv[1] in ("commit", "push"):
        return reviewed(argv[1])
    if argv[0] == "--staged":
        paths = git("diff", "--cached", "--name-only", "--diff-filter=ACMR").splitlines()
        return report(audit(paths, added_lines(git("diff", "--cached", "-U0", "--no-color")), whole_tree=True))
    if argv[0] == "--range" and len(argv) > 1:
        paths = git("diff", "--name-only", "--diff-filter=ACMR", argv[1]).splitlines()
        return report(audit(paths, added_lines(git("diff", "-U0", "--no-color", argv[1])), whole_tree=True))
    if argv[0] == "--all":
        paths = git("ls-files").splitlines()
        return report(audit(paths, all_lines(paths), whole_tree=True))
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
