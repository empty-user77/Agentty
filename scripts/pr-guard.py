#!/usr/bin/env python3
"""pr-guard.py — checks a pull request before anyone reads it, for a repository anybody can fork.

    scripts/pr-guard.py --repo <checkout> --base <sha> --head <sha> [--outside]

Reads the pull request's changes with git (`base...head`) and never runs any of its code. The
workflow (.github/workflows/pr-guard.yml) runs this file from the *base* branch, so a pull request
cannot change the rules it is checked by. Credential formats are gitleaks' job (.gitleaks.toml);
this covers what gitleaks does not:

  PG01  plugin modules (.wasm): published through the Agentty Marketplace only, never from here
  PG02  compiled binaries, executables and archives, whatever their extension says
  PG03  shell and other scripts, and files made executable, outside scripts/ and packaging/
  PG04  personal data: resident registration numbers, phone numbers, card numbers, home paths,
        email addresses, private network addresses
  PG05  workflows that would hand a pull request more than read access or run it somewhere trusted
  PG06  the guards themselves, and the code that decides what agents and plugins may do
  PG07  agents started without their permission checks, and code that raises privileges
  PG08  dependencies from outside crates.io / npm, and install scripts
  PG09  files that must never be committed (keys, env files, transcripts, databases, captures)
  PG10  symbolic links, very large files, invisible or direction-changing Unicode

"block" fails the check. "warn" is shown on the pull request for the reviewer; with --outside (the
author is not the maintainer or a collaborator) the sensitive-path warnings of PG05 / PG06 block too.

    scripts/pr-guard.py --self-test    # checks the rules against built-in samples
"""

import argparse
import os
import re
import subprocess
import sys

# -- What goes where ---------------------------------------------------------------------------------

# Scripts live here and nowhere else; a change to them still needs the maintainer's eye.
SCRIPT_DIRS = re.compile(r"^(scripts|packaging|\.githooks|\.claude/hooks)/")
SCRIPT_EXT = re.compile(r"\.(sh|bash|zsh|fish|ps1|psm1|psd1|cmd|bat|command|vbs|vbe|wsf|applescript|scpt|reg|lnk)$", re.I)
PLUGIN_MODULE = re.compile(r"\.(wasm|wat)$", re.I)
BINARY_EXT = re.compile(
    r"\.(exe|dll|so|dylib|a|lib|o|obj|bin|elf|msi|pkg|dmg|deb|rpm|apk|aab|ipa|appimage|jar|war|class|"
    r"node|pyc|pyo|zip|tar|gz|tgz|bz2|xz|7z|rar|cab|iso|img)$",
    re.I,
)
# Binary files that are fine where they are: images, fonts and icons of the app and the docs.
ASSET_EXT = re.compile(r"\.(png|jpe?g|gif|webp|ico|icns|tiff?|ttf|otf|woff2?|pdf)$", re.I)
ASSET_DIRS = re.compile(r"^(crates/[^/]+/assets|docs|packaging|plugins/[^/]+/(assets|icons?))/")
# Magic numbers of executables and archives, checked on the content itself.
MAGIC = [
    (b"\x7fELF", "an ELF executable"),
    (b"MZ", "a Windows executable"),
    (b"\xcf\xfa\xed\xfe", "a Mach-O executable"),
    (b"\xce\xfa\xed\xfe", "a Mach-O executable"),
    (b"\xca\xfe\xba\xbe", "a Mach-O universal binary or Java class"),
    (b"\x00asm", "a WebAssembly module"),
    (b"PK\x03\x04", "a zip archive"),
    (b"\x1f\x8b", "a gzip archive"),
    (b"7z\xbc\xaf\x27\x1c", "a 7-Zip archive"),
    (b"Rar!", "a RAR archive"),
]

FORBIDDEN_FILES = [
    (r"(^|/)\.env(\.(?!example$|sample$)[^/]+)?$", "an environment file"),
    (r"(^|/)\.envrc$", "a direnv file"),
    (r"\.(pem|p12|pfx|key|p8|keystore|jks|mobileprovision|provisionprofile|kdbx)$", "key or signing material"),
    (r"(^|/)id_(rsa|dsa|ecdsa|ed25519)(\.pub)?$", "an SSH key"),
    (r"(^|/)\.(npmrc|netrc|pypirc|git-credentials)$", "a credentials file"),
    (r"(^|/)(credentials|service[-_]?account[^/]*|client_secret[^/]*)\.json$", "a cloud credentials file"),
    (r"(^|/)\.(aws|ssh|gnupg|kube)/", "a credentials folder"),
    (r"(^|/)\.docker/config\.json$", "registry credentials"),
    (r"(^|/)(\.vercel|\.supabase|supabase/\.temp|\.agentty[^/]*)/", "local tool state"),
    (r"(^|/)\.claude/(settings\.local\.[^/]+|projects/)", "personal Claude Code state"),
    (r"\.jsonl$", "a conversation transcript"),
    (r"\.(sqlite3?|db|db-journal)$", "a database"),
    (r"\.(har|pcap|pcapng)$", "captured traffic"),
    (r"\.tfstate(\.backup)?$", "Terraform state"),
    (r"\.(keychain|keychain-db)$", "a keychain"),
    (r"\.(crash|ips|dmp)$", "a crash report"),
]

# Files that decide what a pull request, an agent or a plugin may do.
GUARD_PATHS = re.compile(
    r"^(\.github/|\.githooks/|\.claude/(hooks|settings\.json)|\.gitleaks\.toml$|\.gitignore$|"
    r"scripts/(check-secrets|security-audit|pr-guard|test_pr_guard)\.py$|CLAUDE\.md$|"
    r"rust-toolchain\.toml$)"
)
SENSITIVE_CODE = re.compile(
    r"^crates/(agentty-bridge/src/(plugins/(manifest|store|net|market)|connectors|secret_store|update|fsutil|"
    r"agent_auth|extensions|idea)\.rs|agentty-app/src/(agent_signal|instance|launch|browser_mcp|browser_cli|"
    r"plugins/(mod|process|link)|workbench/(plugin_host|browser_control|db_page|update))\.rs)$"
)

# -- Content rules -----------------------------------------------------------------------------------

FAKE = re.compile(r"(?i)example|not[_-]?a[_-]?real|fake|dummy|placeholder|sample|redacted|xxxx|your[_-]|test")
DOC_PATH = re.compile(r"\.(md|txt)$|^docs/")


def luhn(digits):
    total, parity = 0, len(digits) % 2
    for i, ch in enumerate(digits):
        d = int(ch)
        if i % 2 == parity:
            d *= 2
            if d > 9:
                d -= 9
        total += d
    return total % 10 == 0


def card_numbers(line):
    for match in re.finditer(r"\b(?:\d[ -]?){13,19}\b", line):
        digits = re.sub(r"\D", "", match.group(0))
        if 13 <= len(digits) <= 19 and len(set(digits)) > 3 and luhn(digits):
            yield match.group(0)


RRN = re.compile(r"\b\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])-?[1-4]\d{6}\b")
PHONE_KR = re.compile(r"(?<![\d.])(?:\+82[- ]?1[016789]|01[016789])[- .]?\d{3,4}[- .]?\d{4}(?![\d.])")
PHONE_INTL = re.compile(r"(?<![\w.])\+\d{1,3}[- ]\(?\d{2,4}\)?[- ]\d{3,4}[- ]\d{3,4}(?![\w.])")
# A home folder with a real name in it, written any way a path appears in code: /Users/x, /home/x,
# C:\\Users\\x, the doubled backslashes of a string literal, and JSON's escaped \\/Users\\/x.
HOME_SEP = r"(?:\\/|\\{1,2}|/)"
HOME = re.compile(
    HOME_SEP + r"(?:Users|home)" + HOME_SEP
    + r"(?!(?:me|you|user|username|name|example|someone|Shared|runner|runneradmin|Public|Default|All Users)\b)"
    + r"(?![<$%{*])[A-Za-z0-9._-]{2,}"
)
EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")
EMAIL_OK = re.compile(
    r"(?i)@(([a-z0-9-]+\.)*example\.(com|org|net)|anthropic\.com|users\.noreply\.github\.com|github\.com|"
    r"localhost|test)$|^git@|^noreply@|^yongyongdev@gmail\.com$"
)
PRIVATE_IP = re.compile(r"\b(?:10(?:\.\d{1,3}){3}|192\.168(?:\.\d{1,3}){2}|172\.(?:1[6-9]|2\d|3[01])(?:\.\d{1,3}){2})\b")
BIDI = re.compile("[\u202a-\u202e\u2066-\u2069]")
INVISIBLE = re.compile("[\u200b-\u200d\u2060\ufeff\xad]")

# (id, rule, severity, paths it applies to, why)
BYPASS = (r"--dangerously-skip-permissions|bypassPermissions|dangerouslyDisableSandbox|--dangerously-bypass-approvals|"
          r"danger-full-access|--yolo\b|skipDangerousModePermissionPrompt|approval_policy\s*=\s*[\"']never")
PIPE_TO_SHELL = r"(?i)\b(curl|wget|iwr|Invoke-WebRequest)\b[^\n|]*\|\s*(sudo\s+)?(ba|z|da)?sh\b|\|\s*iex\b|Invoke-Expression"
# Where Agentty builds an agent's command line or a plugin runs: a bypass there is never a user's choice.
AGENT_LAUNCH = r"^(crates/agentty-app/src/launch\.rs|crates/agentty-bridge/src/(idea|handoff|harness)\.rs|crates/agentty-app/src/workbench/(idea|harness|plugin_host)\.rs|plugins/|sdk/)"
# Code that runs: CI, scripts, packaging, plugins. Elsewhere (crates) such text is usually an install hint shown to the user.
RUNS = r"^(\.github/|scripts/|packaging/|plugins/|sdk/)"

# (id, rule, severity, paths it applies to, why). Comment lines and tests are skipped.
CODE_RULES = [
    ("PG07", BYPASS, "block", AGENT_LAUNCH, "an agent started without its permission checks"),
    ("PG07", BYPASS, "warn", r"^(crates|packaging|scripts)/", "mentions starting an agent without its permission checks"),
    ("PG07", PIPE_TO_SHELL, "block", RUNS, "runs whatever a download returns"),
    ("PG07", PIPE_TO_SHELL, "warn", r"^crates/", "pipes a download into a shell (fine as a hint shown to the user; never run it)"),
    ("PG07", r"(?i)\bsudo\b|\bdoas\b|\bsetuid\b|runas\s+/user|Start-Process[^\n]*-Verb\s+RunAs", "warn", r"^(crates|plugins|sdk)/",
     "asks for administrator rights"),
    ("PG07", r"\bchmod\s+(-R\s+)?(\+s|[ug]\+s|0?[0-7]?777|0?[0-7]?666)\b|(from_mode|set_mode|\.mode)\(\s*0o[0-7]?(777|666)\s*\)",
     "block", r"^(crates|plugins|sdk|scripts|packaging)/", "opens files to every user of the computer, or sets setuid"),
    ("PG07", r"\bnew Function\s*\(|\beval\s*\(|child_process|require\(\s*['\"]vm['\"]\s*\)",
     "warn", r"^(plugins|sdk)/.*\.(mjs|js|cjs|ts)$", "runs code built at run time"),
    ("PG07", r"-EncodedCommand|osascript\s+-e|do shell script",
     "warn", r"^(crates|plugins|sdk)/", "builds a command for another interpreter; check what reaches it"),
    ("PG08", r"^\s*[A-Za-z0-9_-]+\s*=\s*\{[^}]*\bgit\s*=", "block", r"(^|/)Cargo\.toml$",
     "a dependency straight from a Git repository, outside crates.io's review and yanking"),
    ("PG08", r"^\s*\[patch\.|^\s*\[replace\]", "block", r"(^|/)Cargo\.toml$", "replaces a crate for the whole build"),
    ("PG08", r"^source = \"(?!registry\+https://github\.com/rust-lang/crates\.io-index\")", "block", r"(^|/)Cargo\.lock$",
     "a crate from somewhere other than crates.io"),
    ("PG08", r"\"(preinstall|install|postinstall|prepare|prepublish)\"\s*:", "block", r"(^|/)package\.json$",
     "runs on `npm install`, on every machine that installs it"),
    ("PG08", r"\"(resolved)\"\s*:\s*\"(?!https://registry\.npmjs\.org/)", "block", r"(^|/)package-lock\.json$",
     "a package from somewhere other than the npm registry"),
]
COMMENT = re.compile(r"^\s*(//|#(?!\[)|\*|/\*|--\s|REM\b)")
TEST_PATH = re.compile(r"(^|/)(tests?|__tests__|fixtures)/|\.test\.[a-z]+$|_test\.(rs|py)$")
CARGO_PATH_DEP = re.compile(r"\bpath\s*=\s*\"([^\"]+)\"")
COMPILED_RULES = [(pid, re.compile(rx), sev, re.compile(where), why) for pid, rx, sev, where, why in CODE_RULES]
# Where a rule's own words may appear: this guard, the other scanners, the docs.
RULE_EXEMPT = re.compile(r"^(scripts/(pr-guard|test_pr_guard|security-audit|check-secrets)\.py|\.claude/hooks/|\.claude/skills/|CLAUDE\.md|docs/|\.gitleaks\.toml)")

WORKFLOW = re.compile(r"^\.github/workflows/[^/]+\.ya?ml$")
WORKFLOW_RULES = [
    (r"^\s*(pull_request_target|workflow_run|issue_comment|pull_request_review_comment)\s*:", "a trigger that runs with the base repository's rights on a stranger's event"),
    (r"permissions:\s*write-all|^\s*[a-z-]+:\s*write\b", "write access for a workflow"),
    (r"\$\{\{\s*secrets\.", "a secret handed to a workflow (this repository keeps none)"),
    (r"runs-on:[^\n]*self-hosted", "a self-hosted runner: somebody's own machine"),
    (r"uses:\s*(?!\./)(?!docker://)[^@\s]+@(?![0-9a-f]{40}\b)", "an action not pinned to a full commit SHA"),
    (r"\$\{\{\s*github\.(head_ref|event\.(pull_request\.(title|body|head\.ref|head\.label)|issue\.(title|body)|comment\.body|review\.body))",
     "pull request text written into a workflow, where it can become a command"),
    (r"(?i)persist-credentials:\s*true", "leaves the token in the checkout for the pull request's code"),
]
WORKFLOW_COMPILED = [(re.compile(rx, re.M), why) for rx, why in WORKFLOW_RULES]

MAX_BYTES = 5 * 1024 * 1024
WARN_BYTES = 1024 * 1024

# -- Git -----------------------------------------------------------------------------------------------


class Repo:
    def __init__(self, path, base, head):
        self.path, self.base, self.head = path, base, head

    def git(self, *args, binary=False):
        out = subprocess.run(["git", "-C", self.path, *args], capture_output=True, check=True)
        return out.stdout if binary else out.stdout.decode("utf-8", "replace")

    def changes(self):
        """(status, path, old mode, new mode) for every file the pull request touches."""
        raw = self.git("diff", "--raw", "-z", "--no-renames", f"{self.base}...{self.head}")
        parts = raw.split("\0")
        out = []
        i = 0
        while i + 1 < len(parts):
            meta, path = parts[i], parts[i + 1]
            i += 2
            fields = meta.lstrip(":").split()
            if len(fields) < 5:
                continue
            old_mode, new_mode, status = fields[0], fields[1], fields[4]
            out.append((status[0], path, old_mode, new_mode))
        return out

    def added_lines(self):
        """{path: [(line number, text)]} of the lines the pull request adds."""
        diff = self.git("diff", "-U0", "--no-renames", "--no-color", f"{self.base}...{self.head}")
        lines, path, number = {}, None, 0
        for line in diff.split("\n"):
            if line.startswith("+++ "):
                path = line[6:] if line.startswith("+++ b/") else None
            elif line.startswith("@@"):
                match = re.search(r"\+(\d+)", line)
                number = int(match.group(1)) if match else 0
            elif path and line.startswith("+") and not line.startswith("+++"):
                lines.setdefault(path, []).append((number, line[1:]))
                number += 1
        return lines

    def blob(self, path):
        try:
            return self.git("cat-file", "blob", f"{self.head}:{path}", binary=True)
        except subprocess.CalledProcessError:
            return b""


# -- Checks --------------------------------------------------------------------------------------------


def check(changes, added, blob, outside):
    findings = []

    def add(severity, rule, path, line, message):
        findings.append((severity, rule, path, line, message))

    for status, path, old_mode, new_mode in changes:
        if status == "D":
            if GUARD_PATHS.match(path):
                add("block", "PG06", path, 0, "deletes a guard file")
            continue
        content = blob(path)
        # PG10 links, size
        if new_mode == "120000":
            add("block", "PG10", path, 0, "a symbolic link (it can point anywhere on the machine that checks it out)")
            continue
        if len(content) > MAX_BYTES:
            add("block", "PG10", path, 0, f"{len(content) // 1024 // 1024} MB: large files belong in a release, not the repository")
        elif len(content) > WARN_BYTES:
            add("warn", "PG10", path, 0, f"{len(content) // 1024} KB file")
        # PG09 forbidden files
        for rx, what in FORBIDDEN_FILES:
            if re.search(rx, path):
                add("block", "PG09", path, 0, f"{what} must never be committed")
        # PG01 plugin modules
        if PLUGIN_MODULE.search(path) or content.startswith(b"\x00asm"):
            add("block", "PG01", path, 0, "a plugin module: plugins are published through the Agentty Marketplace only")
            continue
        # PG02 binaries
        magic = next((what for sig, what in MAGIC if content.startswith(sig)), None)
        if magic and not ASSET_EXT.search(path):
            add("block", "PG02", path, 0, f"{magic}: compiled code and archives are not accepted")
        elif BINARY_EXT.search(path):
            add("block", "PG02", path, 0, "a binary or archive by its extension")
        elif b"\0" in content[:8000] and not (ASSET_EXT.search(path) and ASSET_DIRS.match(path)):
            add("block", "PG02", path, 0, "a binary file outside the app's and the docs' assets")
        # PG03 scripts and executables
        executable = new_mode == "100755"
        script = SCRIPT_EXT.search(path) or content.startswith(b"#!")
        if script or executable:
            what = "a script" if script else "an executable file"
            if not SCRIPT_DIRS.match(path):
                add("block", "PG03", path, 0, f"{what} outside scripts/ and packaging/")
            else:
                add("block" if outside else "warn", "PG03", path, 0, f"{what}: it runs on the maintainer's and users' machines")
        if old_mode == "100644" and new_mode == "100755" and not SCRIPT_DIRS.match(path):
            add("block", "PG03", path, 0, "made executable outside scripts/ and packaging/")
        # PG05 / PG06 sensitive paths
        if GUARD_PATHS.match(path):
            add("block" if outside else "warn", "PG06", path, 0, "changes a guard, the CI or the rules agents follow: maintainer review")
        elif SENSITIVE_CODE.match(path):
            add("warn", "PG06", path, 0, "security-sensitive code (permissions, credentials, local interfaces, updates): review closely")
        if path.endswith("build.rs") or re.search(r"proc-macro\s*=\s*true", content.decode("utf-8", "replace")):
            add("warn", "PG08", path, 0, "code that runs at build time")

    for path, lines in added.items():
        exempt = RULE_EXEMPT.match(path)
        doc = DOC_PATH.search(path)
        for number, text in lines:
            fake = FAKE.search(text)
            # PG10 Unicode
            if BIDI.search(text):
                add("block", "PG10", path, number, "direction-changing Unicode: the code reads differently than it runs")
            elif INVISIBLE.search(text) and not doc:
                add("warn", "PG10", path, number, "an invisible character")
            # PG04 personal data
            if not fake:
                if RRN.search(text):
                    add("block", "PG04", path, number, "a resident registration number")
                if PHONE_KR.search(text) or PHONE_INTL.search(text):
                    add("block", "PG04", path, number, "a phone number")
                if any(card_numbers(text)):
                    add("block", "PG04", path, number, "a payment card number")
                if not exempt and HOME.search(text):
                    add("block", "PG04", path, number, "a real home folder path (use /Users/me or ~)")
                for email in EMAIL.findall(text):
                    if not EMAIL_OK.search(email) and not path.startswith("crates/agentty-app/assets/licenses/"):
                        add("warn", "PG04", path, number, "an email address: fine for a public contact, never somebody else's")
                        break
                if PRIVATE_IP.search(text) and not doc:
                    add("warn", "PG04", path, number, "a private network address")
            # PG05 workflows
            if WORKFLOW.match(path):
                for rx, why in WORKFLOW_COMPILED:
                    if rx.search(text):
                        add("block", "PG05", path, number, why)
            # PG07 / PG08 code
            if path.endswith("Cargo.toml"):
                for dep in CARGO_PATH_DEP.findall(text):
                    inside = os.path.normpath(os.path.join(os.path.dirname(path), dep))
                    if dep.startswith("/") or inside == ".." or inside.startswith("../"):
                        add("block", "PG08", path, number, "a dependency from outside the repository")
            if exempt or doc or TEST_PATH.search(path) or COMMENT.match(text):
                continue
            for pid, rx, severity, where, why in COMPILED_RULES:
                if where.search(path) and rx.search(text):
                    add(severity, pid, path, number, why)
    # A line that already blocks under a rule needs no warning under the same rule.
    blocked = {(rule, path, line) for severity, rule, path, line, _ in findings if severity == "block"}
    return [f for f in findings if f[0] == "block" or (f[1], f[2], f[3]) not in blocked]


# -- Output ----------------------------------------------------------------------------------------------


def escape(text):
    return text.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def report(findings):
    blocks = [f for f in findings if f[0] == "block"]
    for severity, rule, path, line, message in findings:
        level = "error" if severity == "block" else "warning"
        where = f"file={escape(path)}" + (f",line={line}" if line else "")
        print(f"::{level} {where},title={rule}::{escape(message)}")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as out:
            out.write("## PR guard\n\n")
            if not findings:
                out.write("Nothing found.\n")
            else:
                out.write("| | Rule | File | What |\n|---|---|---|---|\n")
                for severity, rule, path, line, message in findings:
                    mark = "❌" if severity == "block" else "⚠️"
                    place = f"`{path}`" + (f":{line}" if line else "")
                    out.write(f"| {mark} | {rule} | {place} | {message} |\n")
    print(f"pr-guard: {len(blocks)} blocking, {len(findings) - len(blocks)} to review")
    return 1 if blocks else 0


# -- Self-test ---------------------------------------------------------------------------------------------


def self_test():
    """Runs the rules on samples built here (credential- and PII-shaped values are assembled at run
    time, so this file never holds one)."""
    rrn = "90" + "0101" + "-" + "1" + "234567"
    phone = "010" + "-" + "1234" + "-" + "5678"
    body = "4539" + "1488" + "0343" + "646"
    check_digit = next(str(d) for d in range(10) if luhn(body + str(d)))
    card = " ".join((body + check_digit)[i:i + 4] for i in range(0, 16, 4))
    home = "/Users/" + "alice/projects"
    cases = [
        # (changes, added lines, blobs, outside, expected (severity, rule) set)
        ([("A", "plugins/x/x.wasm", "000000", "100644")], {}, {"plugins/x/x.wasm": b"\x00asm\x01"}, False, {("block", "PG01")}),
        ([("A", "tools/run.sh", "000000", "100755")], {}, {"tools/run.sh": b"#!/bin/sh\n"}, False, {("block", "PG03")}),
        ([("M", "scripts/build.sh", "100755", "100755")], {}, {"scripts/build.sh": b"#!/bin/sh\n"}, False, {("warn", "PG03")}),
        ([("M", "scripts/build.sh", "100755", "100755")], {}, {"scripts/build.sh": b"#!/bin/sh\n"}, True, {("block", "PG03")}),
        ([("A", "crates/x/src/data.txt", "000000", "100644")], {}, {"crates/x/src/data.txt": b"MZ\x90\x00"}, False, {("block", "PG02")}),
        ([("A", "crates/agentty-app/assets/icon.png", "000000", "100644")], {}, {"crates/agentty-app/assets/icon.png": b"\x89PNG\r\n\x1a\n\0"}, False, set()),
        ([("A", ".env.production", "000000", "100644")], {}, {".env.production": b"A=1"}, False, {("block", "PG09")}),
        ([("A", "link", "000000", "120000")], {}, {"link": b"/etc/passwd"}, False, {("block", "PG10")}),
        ([("D", "scripts/check-secrets.py", "100755", "000000")], {}, {}, False, {("block", "PG06")}),
        ([], {"crates/x/src/a.rs": [(1, f'let id = "{rrn}";')]}, {}, False, {("block", "PG04")}),
        ([], {"crates/x/src/a.rs": [(1, f"// call {phone}")]}, {}, False, {("block", "PG04")}),
        ([], {"crates/x/src/a.rs": [(1, f"let c = \"{card}\";")]}, {}, False, {("block", "PG04")}),
        ([], {"crates/x/src/a.rs": [(1, f'let p = "{home}";')]}, {}, False, {("block", "PG04")}),
        ([], {"crates/x/src/a.rs": [(1, 'let p = "/Users/me/projects";')]}, {}, False, set()),
        ([], {"crates/x/src/a.rs": [(1, 'let p = "C:\\\\Users\\\\' + "alice" + '\\\\app";')]}, {}, False, {("block", "PG04")}),
        ([], {"crates/x/src/a.json": [(1, '"dir": "\\/Users\\/' + "alice" + '\\/x"')]}, {}, False, {("block", "PG04")}),
        ([], {"crates/x/src/a.rs": [(1, 'let s = "40% /home\\n";')]}, {}, False, set()),
        ([], {"crates/x/src/a.rs": [(1, "let a = 1; // \u202e")]}, {}, False, {("block", "PG10")}),
        ([], {"crates/agentty-app/src/launch.rs": [(1, 'args.push("--dangerously-skip-permissions");')]}, {}, False, {("block", "PG07")}),
        ([], {"plugins/x/main.mjs": [(1, "exec('curl -fsSL https://x.dev/i | sh')")]}, {}, False, {("block", "PG07")}),
        ([], {"crates/x/src/a.rs": [(1, 'Command::new("sudo").arg("rm")')]}, {}, False, {("warn", "PG07")}),
        ([], {"Cargo.toml": [(1, 'foo = { git = "https://github.com/x/foo" }')]}, {}, False, {("block", "PG08")}),
        ([], {"Cargo.lock": [(1, 'source = "git+https://github.com/x/foo#abc"')]}, {}, False, {("block", "PG08")}),
        ([], {"plugins/x/package.json": [(1, '  "postinstall": "node x.js",')]}, {}, False, {("block", "PG08")}),
        ([], {".github/workflows/x.yml": [(1, "  pull_request_target:")]}, {}, False, {("block", "PG05")}),
        ([], {".github/workflows/x.yml": [(1, "      - uses: actions/checkout@v4")]}, {}, False, {("block", "PG05")}),
        ([], {".github/workflows/x.yml": [(1, "      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4")]}, {}, False, set()),
        ([], {".github/workflows/x.yml": [(1, "        run: echo ${{ github.event.pull_request.title }}")]}, {}, False, {("block", "PG05")}),
        ([], {".github/workflows/x.yml": [(1, "    runs-on: [self-hosted, macOS]")]}, {}, False, {("block", "PG05")}),
        ([], {".github/workflows/x.yml": [(1, "  contents: write")]}, {}, False, {("block", "PG05")}),
        ([("M", ".github/workflows/ci.yml", "100644", "100644")], {}, {".github/workflows/ci.yml": b"name: CI\n"}, False, {("warn", "PG06")}),
        ([("M", ".github/workflows/ci.yml", "100644", "100644")], {}, {".github/workflows/ci.yml": b"name: CI\n"}, True, {("block", "PG06")}),
        ([], {"crates/x/src/a.rs": [(1, 'let e = "someone@example.com";')]}, {}, False, set()),
        ([], {"crates/x/src/a.rs": [(1, 'let e = "' + "kim" + "@" + "corp-mail.io" + '";')]}, {}, False, {("warn", "PG04")}),
        ([], {"crates/x/src/a.rs": [(1, 'const TOKEN: &str = "example_not_a_real_value";')]}, {}, False, set()),
        ([], {"crates/x/Cargo.toml": [(1, 'agentty-bridge = { path = "../agentty-bridge" }')]}, {}, False, set()),
        ([], {"crates/x/Cargo.toml": [(1, 'evil = { path = "../../../elsewhere" }')]}, {}, False, {("block", "PG08")}),
        ([], {"crates/x/src/a.rs": [(1, "let m = meta.permissions().mode() & 0o777;")]}, {}, False, set()),  # audit: ok — a self-test sample
        ([], {"crates/x/src/a.rs": [(1, "std::fs::set_permissions(p, Permissions::from_mode(0o777))")]}, {}, False, {("block", "PG07")}),  # audit: ok — a self-test sample
        ([], {"crates/agentty-app/src/shell_integration.rs": [(1, "//! `zzzz = --dangerously-skip-permissions` for claude")]}, {}, False, set()),
        ([], {"crates/agentty-app/src/setup_check.rs": [(1, '"curl -fsSL https://sh.rustup.rs | sh"')]}, {}, False, {("warn", "PG07")}),
    ]
    failed = 0
    for number, (changes, added, blobs, outside, expected) in enumerate(cases, 1):
        got = {(f[0], f[1]) for f in check(changes, added, lambda p: blobs.get(p, b""), outside)}
        if got != expected:
            failed += 1
            print(f"case {number}: expected {sorted(expected)}, got {sorted(got)}")
    print(f"pr-guard self-test: {len(cases) - failed}/{len(cases)} passed")
    return 1 if failed else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--repo", default=".")
    parser.add_argument("--base")
    parser.add_argument("--head")
    parser.add_argument("--outside", action="store_true", help="the author is not the maintainer or a collaborator")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if not (args.base and args.head):
        parser.error("--base and --head are required")
    repo = Repo(args.repo, args.base, args.head)
    return report(check(repo.changes(), repo.added_lines(), repo.blob, args.outside))


if __name__ == "__main__":
    sys.exit(main())
