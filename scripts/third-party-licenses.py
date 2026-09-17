#!/usr/bin/env python3
"""Writes the license texts of everything compiled into the app bundle.

Covers every crate in the release dependency graph (normal dependencies for the macOS target) plus the
bundled fonts, icons and logos. Identical license texts are printed once with the list of crates they apply to.

Usage: scripts/third-party-licenses.py <out.txt> [target-triple]
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
    ("Simple Icons brand logos (CC0 1.0)", "crates/agentty-app/assets/logos/LICENSE.txt"),
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
        yield pkg["name"], pkg["version"], pkg.get("license") or "see license file", pathlib.Path(pkg["manifest_path"]).parent, pkg.get("license_file")


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


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    target = sys.argv[2] if len(sys.argv) > 2 else "aarch64-apple-darwin"
    groups, missing, crates = {}, [], []
    for name, version, license, directory, license_file in packages(target):
        crates.append(f"{name} {version} — {license}")
        texts = license_texts(directory, license_file)
        if not texts:
            missing.append(f"{name} {version} ({license})")
        for text in texts:
            groups.setdefault(text, []).append(f"{name} {version}")

    out = ["Agentty third-party licenses", "=" * 28, "",
           "Agentty is licensed under GPL-3.0-or-later (see LICENSE.txt). It includes the components below.", ""]
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
