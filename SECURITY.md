# Security Policy

## Supported versions

Agentty is pre-1.0; security fixes land on the `main` branch and the latest release.

## Reporting a vulnerability

Please **do not** open a public issue. Use GitHub's
[private vulnerability reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)
on this repository, including:

- affected version / commit,
- steps to reproduce and impact,
- any suggested fix.

We aim to acknowledge reports within 3 business days and to ship a fix or mitigation as soon as possible.

## Security model (summary)

- Agentty runs locally. Agent transcripts are read from disk and never uploaded.
- Agent status hooks connect to a per-process Unix socket under `~/.agentty/run/`.
- Agentty does not modify Claude Code or Codex configuration files; hooks are passed per launch.
- Secrets for external connectors (planned) are encrypted at rest with a key held in the macOS Keychain.
