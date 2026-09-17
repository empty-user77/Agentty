#!/usr/bin/env bash
# bump-version.sh patch|minor|major|x.y.z — updates the workspace version in Cargo.toml.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
current="$(grep -m1 -E '^version = "' "$ROOT/Cargo.toml" | sed -E 's/version = "(.*)"/\1/')"
IFS=. read -r major minor patch <<< "$current"
case "${1:-patch}" in
  patch) next="$major.$minor.$((patch + 1))" ;;
  minor) next="$major.$((minor + 1)).0" ;;
  major) next="$((major + 1)).0.0" ;;
  [0-9]*.[0-9]*.[0-9]*) next="$1" ;;
  *) echo "usage: $0 patch|minor|major|x.y.z" >&2; exit 1 ;;
esac
python3 - "$ROOT/Cargo.toml" "$current" "$next" <<'PY'
import sys; p, cur, nxt = sys.argv[1:]
s = open(p).read(); s = s.replace(f'version = "{cur}"', f'version = "{nxt}"', 1); open(p, 'w').write(s)
PY
(cd "$ROOT" && cargo metadata --no-deps --format-version 1 >/dev/null)
echo "$current → $next"
