---
title: Publishing a plugin
description: List a plugin in the marketplace, or share it yourself as a Git repository or a folder.
---

There are two ways to get a plugin to people, and which one is open to you depends on what kind of plugin it is.

| | |
|---|---|
| **The marketplace** | A WebAssembly module whose source is public. Listed in **Plugins → Marketplace**, installed in one click, checked against a checksum. |
| **Yourself** | Any plugin, including one that runs as a program. People install it from a Git repository or a folder, having chosen the source themselves. |

## The marketplace

[Agentty-Marketplace](https://github.com/empty-user77/Agentty-Marketplace) is the list Agentty reads. Plugins are added by pull request. Agentty's own plugins are there too, on the same footing — nothing is bundled into the application.

A plugin in the list is **a WebAssembly module with its source in the open**. That is the whole rule, and both halves matter:

- **WebAssembly**, because Agentty runs it itself and it reaches only what the protocol gives it. A plugin that runs as a program has everything you have, and Agentty will not install one from a list on the internet.
- **Source in the open**, because the module is a binary. Anyone can read what it was built from, and build it again.

### What installing does

1. Agentty reads `index.json` over HTTPS. Every entry is checked again here — its id, its text, its permissions, the host its module comes from — and one that does not check out is left out of the list rather than shown.
2. The Plugins page shows what the plugin is, where its source is, its licence, the size of the module and its checksum, and **what it may do** as full sentences under Permissions.
3. On **Install**, Agentty downloads the module and weighs it against that checksum. Nothing reaches the plugins folder before they match.

### Submitting one

1. **Build and publish the module.** A GitHub release of your plugin's repository is the usual place.

   ```bash
   cargo build --release --target wasm32-unknown-unknown
   cp target/wasm32-unknown-unknown/release/your_plugin.wasm your-plugin.wasm
   shasum -a 256 your-plugin.wasm
   wc -c your-plugin.wasm
   ```

2. **Write the entry.** Copy `plugins/_template.json` to `plugins/<your-plugin-id>.json`. The id matches the file name and the `id` in your `agentty-plugin.json`.

   ```json
   {
     "id": "hello-world",
     "name": "Hello World",
     "version": "0.1.0",
     "publisher": "Your Name",
     "description": "One sentence about what it does.",
     "icon": "sparkles",
     "license": "MIT",
     "source": "https://github.com/you/agentty-hello-world",
     "build": {
       "repository": "https://github.com/you/agentty-hello-world",
       "rev": "3f2b1c9e4a7d05b8c6e1f0a2d4b83c7e9015d6af",
       "path": ".",
       "toolchain": "1.98.1",
       "artifact": "target/wasm32-unknown-unknown/release/hello_world.wasm"
     },
     "keywords": ["example"],
     "apiVersion": 1,
     "surface": "sidebar",
     "mode": "push",
     "permissions": [],
     "module": {
       "url": "https://github.com/you/agentty-hello-world/releases/download/v0.1.0/hello-world.wasm",
       "sha256": "…64 hex characters…",
       "size": 93292
     }
   }
   ```

3. **Check it, then open a pull request.** CI runs the same checks, and refuses the entry unless the entry, the module it points at and a fresh build of your source all agree.

   ```bash
   python3 scripts/validate.py              # every entry's shape, hosts, permissions and build block
   python3 scripts/validate.py --download   # also fetch each module and check its checksum
   python3 scripts/validate.py --source     # also check the source is readable by anyone
   python3 scripts/verify_build.py          # build your source again and compare it to the checksum
   ```

`verify_build.py` needs Docker: it builds the module in a `rust` image pinned by digest, so the bytes do not depend on the machine that built them. That is what lets CI arrive at the same checksum you did.

### The entry's fields

| Field | |
|---|---|
| `id` | 2–40 characters, `a-z 0-9 -`; the file is `plugins/<id>.json` |
| `name`, `version`, `description` | shown in Agentty. `name` up to 60 characters, `description` up to 300, `version` is `major.minor.patch` |
| `publisher`, `license` | who made it, and under what licence |
| `source` | the public repository the module is built from — **required**. On `github.com`, `gitlab.com`, `codeberg.org` or `git.sr.ht`; it does not have to be GitHub. It has to be the same repository as `build.repository`, so the code an entry links to is the code it ships |
| `build` | **required** — how to build that module again: `repository` (the clone URL), `rev` (the full 40-character commit, not a tag or a branch, which can be moved afterwards), `path` (the plugin's directory in the repository, or `.`), `toolchain` (the Rust release the marketplace builds with) and `artifact` (the `.wasm` the build writes, relative to `path`). None of it is a command — what is run on it is fixed in `scripts/verify_build.py` |
| `homepage`, `keywords`, `icon` | optional; the icon is a name from Agentty's set |
| `apiVersion` | the plugin protocol the module is built against; leave it out for `1` |
| `surface` | where its icon sits: `sidebar`, `pane` (default) or `status` |
| `mode` | how its panel opens: `push` (default), `overlay`, `window` or `full` |
| `permissions` | what it asks for — shown before anyone installs it, and again on an update that asks for more |
| `module.url` | `https://` on `github.com`, `raw.githubusercontent.com` or `objects.githubusercontent.com`. Put the version in the path so a release cannot be swapped underneath — a convention, not a check |
| `module.sha256` | the checksum; Agentty refuses a download that does not match, and refuses bytes that are not a WebAssembly module even when it does |
| `module.size` | its exact length in bytes, up to 8 MB. Not a ceiling: a download of any other length is refused, so this changes with every build |

A listed plugin carries no logo of its own in the entry. If you want artwork instead of an icon, put it in the module's `agentty.logo` section — it is then covered by `module.sha256` like everything else, and nothing is fetched from your server when somebody installs the plugin. See [Logo](/docs/plugin-rust#logo).

### What gets a plugin refused

- A module CI cannot arrive at again by building `build.repository` at `build.rev`, or a repository nobody can read.
- A manifest at `build.path` that disagrees with the entry — a different id, version, `apiVersion` or set of permissions.
- A checksum that does not match what the URL serves.
- Permissions the plugin does not use, or a description that does not say what it does with them.
- Anything that pretends to be another plugin, another publisher, or Agentty itself.

### Updating

Change `version`, `module.url`, `module.sha256` and `module.size`, and open another pull request. Agentty offers the update to everyone who has it installed.

An update that asks for **more** than the installed version says what it is adding and takes a second press. **Update all** leaves those out rather than taking them quietly, and says how many it left. So growing a plugin's permissions costs you the users who do not look again — write the description so that they do.

If the new version uses something only a newer Agentty has, raise `apiVersion` with it. People on an older Agentty then keep the version they have and are told to update, instead of being handed a module their app cannot run — and Agentty does not count a version it cannot run as an update to one already installed.

Deleting an entry stops Agentty offering the plugin. It stays installed for people who have it, and they can remove it from the Plugins page.

> [!NOTE]
> `AGENTTY_MARKETPLACE_INDEX` points Agentty at another list while you are writing one. A module may be served from a release of a repository, or from the same host as the list itself.

## Sharing it yourself

Nothing above is required to use a plugin. A plugin you keep to yourself, or to your company, never has to pass through that list.

### As a Git repository

Put `agentty-plugin.json` at the **root**. Users paste the `https://` URL under *Build your own* → **Install from Git**.

```
your-plugin/
├── agentty-plugin.json
├── main.mjs
├── agentty-plugin.mjs     the SDK, bundled
├── README.md
└── LICENSE
```

> [!IMPORTANT]
> **Bundle everything the plugin needs to run, including the SDK and any `node_modules`.** Agentty does not run `npm install` — it starts your entry point as it is. A plugin with an unbundled dependency fails on the user's machine with a module-not-found error in its log.

Prefer dependency-free code where you can. The SDK itself has none for exactly this reason.

### As a folder

**Install from Folder…** copies the folder into `~/.agentty/plugins/`. A zip that unpacks to a correctly named folder works the same way — the folder name must equal the `id` in the manifest.

## Versioning

`version` is `major.minor.patch`. Raise it whenever you publish; Agentty compares it to decide whether an installed plugin has an update.

Keep the manifest and the code in step: a permission you stopped using should leave the manifest too, and a command you renamed should not linger in `contributes`.

## Next

- [Rust and WebAssembly](/docs/plugin-rust) — building the module
- [Manifest reference](/docs/plugin-manifest) · [Permissions](/docs/plugin-permissions)
