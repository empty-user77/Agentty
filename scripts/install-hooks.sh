#!/bin/sh
# One-time setup after cloning: enable the repository's git hooks (secret scanning).
set -e
cd "$(git rev-parse --show-toplevel)"
git config core.hooksPath .githooks
chmod +x .githooks/* scripts/check-secrets.py scripts/security-audit.py .claude/hooks/*.py
echo "Git hooks enabled (.githooks): pre-commit and pre-push secret scanning and security audit."
