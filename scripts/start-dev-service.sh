#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UNIT="openreader-dev.service"
NPM_BIN="$(command -v npm)"
NODE_BIN_DIR="$(dirname "$(command -v node)")"
RUNTIME_PATH="$NODE_BIN_DIR:$HOME/.cargo/bin:$HOME/.local/bin:/usr/local/bin:/usr/bin:/bin"

if [ ! -x "$ROOT_DIR/backend/.venv/bin/python" ]; then
  echo "Backend-Venv fehlt. Fuehre zuerst ./scripts/dev.sh aus." >&2
  exit 1
fi

if systemctl --user cat "$UNIT" >/dev/null 2>&1; then
  systemctl --user restart "$UNIT"
else
  systemd-run --user \
    --unit="$UNIT" \
    --collect \
    --property=KillMode=mixed \
    --working-directory="$ROOT_DIR" \
    --setenv="PATH=$RUNTIME_PATH" \
    --setenv="OPENREADER_PYTHON=$ROOT_DIR/backend/.venv/bin/python" \
    "$NPM_BIN" run app:dev
fi

for _ in {1..40}; do
  if curl -fsS http://127.0.0.1:8765/api/health >/dev/null 2>&1; then
    echo "OpenReader laeuft als $UNIT."
    exit 0
  fi
  sleep 0.25
done

echo "OpenReader wurde nicht rechtzeitig bereit." >&2
systemctl --user status "$UNIT" --no-pager >&2 || true
exit 1
