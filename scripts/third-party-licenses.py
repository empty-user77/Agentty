#!/usr/bin/env python3
"""Writes the license texts of everything compiled into the app bundle.

Covers every crate in the release dependency graph (normal dependencies for the macOS target) plus the
bundled fonts, icons, logos and the file editor's syntax definitions. Identical license texts are printed once with the list of crates they apply to.

Usage: scripts/third-party-licenses.py <out.txt> [target-triple]
       scripts/third-party-licenses.py --list [--check]

--list writes crates/agentty-app/assets/licenses/open-source.tsv, the list the app shows under Settings → About →
Open source: every component above and every crate linked into the app on macOS, Windows or Linux, with its version,
license and home page. --check fails when that file is not what --list would write (CI runs it).
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
LICENSE_PREFIXES = ("license", "licence", "copying", "notice", "unlicense")
ASSETS = [
    ("JetBrains Mono (SIL Open Font License 1.1)", "crates/agentty-app/assets/fonts/JetBrainsMono-OFL.txt"),
    ("JetBrainsMono Nerd Font Mono (Nerd Fonts)", "crates/agentty-app/assets/fonts/NerdFonts-LICENSE.txt"),
    ("Lucide icons (ISC)", "crates/agentty-app/assets/icons/LICENSE.txt"),
    # Sublime Text grammars compiled into the app by `two-face` for the file editor's syntax colors.
    ("Syntax definitions of the file editor (bat's set via two-face; MIT, Apache-2.0, BSD)", "crates/agentty-app/assets/licenses/syntax-definitions.md"),
    ("Simple Icons brand logos (CC0 1.0)", "crates/agentty-app/assets/logos/LICENSE.txt"),
    # The markdown preview's renderer, compiled into the app.
    ("markdown-it (MIT)", "crates/agentty-app/assets/markdown-preview/LICENSE-markdown-it.txt"),
    ("highlight.js (BSD-3-Clause)", "crates/agentty-app/assets/markdown-preview/LICENSE-highlight.js.txt"),
    ("DOMPurify (Apache-2.0 OR MPL-2.0)", "crates/agentty-app/assets/markdown-preview/LICENSE-dompurify.txt"),
    ("VS Code markdown preview styles (MIT)", "crates/agentty-app/assets/markdown-preview/LICENSE-vscode.txt"),
]


LIST = ROOT / "crates/agentty-app/assets/licenses/open-source.tsv"
LIST_TARGETS = ("aarch64-apple-darwin", "x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu")
# What the app bundles besides crates, for the in-app list: (name, version, license, home page).
LIST_ASSETS = [
    ("JetBrains Mono", "2.304", "OFL-1.1", "https://github.com/JetBrains/JetBrainsMono"),
    ("JetBrainsMono Nerd Font Mono", "3.5.1", "OFL-1.1 AND MIT", "https://github.com/ryanoasis/nerd-fonts"),
    ("Lucide icons", "", "ISC", "https://lucide.dev"),
    ("Simple Icons", "", "CC0-1.0", "https://simpleicons.org"),
    ("bat syntax definitions", "", "MIT / Apache-2.0 / BSD", "https://github.com/sharkdp/bat"),
    ("markdown-it", "14.3.2", "MIT", "https://github.com/markdown-it/markdown-it"),
    ("highlight.js", "11.12.0", "BSD-3-Clause", "https://github.com/highlightjs/highlight.js"),
    ("DOMPurify", "3.4.16", "Apache-2.0 OR MPL-2.0", "https://github.com/cure53/DOMPurify"),
    ("VS Code markdown preview styles", "", "MIT", "https://github.com/microsoft/vscode"),
]


def packages(target: str):
    """(name, version, license, manifest dir) of every non-workspace crate linked into the app."""
    meta = json.loads(
        subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target], cwd=ROOT)
    )
    by_id = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    workspace = set(meta["workspace_members"])
    seen, stack = set(), list(workspace)
    while stack:
        node = nodes[stack.pop()]
        for dep in node["deps"]:
            normal = any(kind["kind"] is None for kind in dep["dep_kinds"])
            if normal and dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                stack.append(dep["pkg"])
    for pkg_id in sorted(seen - workspace, key=lambda i: (by_id[i]["name"], by_id[i]["version"])):
        pkg = by_id[pkg_id]
        yield pkg["name"], pkg["version"], pkg.get("license") or "see license file", pathlib.Path(pkg["manifest_path"]).parent, pkg.get("license_file"), pkg.get("repository") or pkg.get("homepage")


def license_texts(directory: pathlib.Path, license_file):
    files = [directory / license_file] if license_file else []
    files += sorted(p for p in directory.iterdir() if p.is_file() and p.name.lower().startswith(LICENSE_PREFIXES))
    texts = []
    for path in dict.fromkeys(files):
        try:
            text = path.read_text(errors="replace").strip()
        except OSError:
            continue
        if text and text not in texts:
            texts.append(text)
    return texts


def version_key(version: str):
    """1.10.0 after 1.9.0: numeric parts compared as numbers."""
    import re
    return [int(part) if part.isdigit() else part for part in re.split(r"[.+-]", version)]


def clean(field: str) -> str:
    return " ".join(str(field).split())


def open_source_list() -> str:
    """The in-app list: kind, name, version, license, home page — tab-separated, one component per line."""
    crates = {}
    for target in LIST_TARGETS:
        for name, version, license, _, _, home in packages(target):
            # Old manifests write "MIT/Apache-2.0" for "MIT OR Apache-2.0".
            license = " OR ".join(part.strip() for part in license.split("/"))
            crates[(name, version)] = (license, home or f"https://crates.io/crates/{name}/{version}")
    lines = ["# Generated by scripts/third-party-licenses.py --list. Do not edit."]
    lines += ["\t".join(map(clean, ("asset", *asset))) for asset in LIST_ASSETS]
    lines += ["\t".join(map(clean, ("crate", name, version, license, home))) for (name, version), (license, home) in sorted(crates.items(), key=lambda item: (item[0][0], version_key(item[0][1])))]
    return "\n".join(lines) + "\n"


def main():
    if sys.argv[1:2] == ["--list"]:
        text = open_source_list()
        if "--check" in sys.argv:
            if not LIST.exists() or LIST.read_text() != text:
                sys.exit(f"{LIST.relative_to(ROOT)} is out of date: run scripts/third-party-licenses.py --list")
            return print(f"{LIST.relative_to(ROOT)} is current")
        LIST.write_text(text)
        return print(f"wrote {LIST.relative_to(ROOT)} ({text.count(chr(10)) - 1} components)")
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    target = sys.argv[2] if len(sys.argv) > 2 else "aarch64-apple-darwin"
    groups, missing, crates = {}, [], []
    for name, version, license, directory, license_file, _ in packages(target):
        crates.append(f"{name} {version} — {license}")
        texts = license_texts(directory, license_file)
        if not texts:
            missing.append(f"{name} {version} ({license})")
        for text in texts:
            groups.setdefault(text, []).append(f"{name} {version}")

    out = ["Agentty third-party licenses", "=" * 28, "",
           "Agentty is licensed under Apache-2.0 (see LICENSE.txt). It includes the components below.", ""]
    for title, path in ASSETS:
        out += ["-" * 100, title, "-" * 100, (ROOT / path).read_text().strip(), ""]
    out += ["-" * 100, f"Rust crates ({len(crates)})", "-" * 100, *crates, ""]
    for text, users in groups.items():
        out += ["-" * 100, "Applies to: " + ", ".join(users), "-" * 100, text, ""]
    if missing:
        out += ["-" * 100, "Crates without a bundled license file (license per their SPDX expression above):",
                *missing, ""]
    pathlib.Path(sys.argv[1]).write_text("\n".join(out) + "\n")
    print(f"{len(crates)} crates, {len(groups)} distinct license texts, {len(missing)} without a license file")


if __name__ == "__main__":
    main()
