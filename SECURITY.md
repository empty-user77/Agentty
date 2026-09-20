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
- Agent status hooks connect to a per-process Unix socket (mode 0600) in the user's private temporary directory.
- Agentty does not rewrite Claude Code or Codex configuration files; hooks and the statusline wrapper are passed per
  launch, and the wrapper still runs the user's own statusline.
- API connector credentials are stored in the macOS Keychain, never in `connectors.json`, agent configs or logs.
- Release builds send anonymous usage events (no paths, commands, prompts or output) unless the user turns them off
  in Settings → General or sets `DO_NOT_TRACK=1`; see `docs/metrics.md`.
