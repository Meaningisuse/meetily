#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
temp_root="$(mktemp -d)"
trap 'rm -rf "$temp_root"' EXIT

capture="$temp_root/args"
env_capture="$temp_root/env"
fake_client="$temp_root/tunnel-client"
cat >"$fake_client" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" >"$CAPTURE_ARGS"
[[ "${CONTROL_PLANE_API_KEY:-}" == "test-control-plane-key" ]]
[[ "${MEETILY_AUTOMATION_CLIENT_ID:-}" == "test-client-id" ]]
printf 'credentials received via environment\n' >"$CAPTURE_ENV"
SH
chmod +x "$fake_client"

CONTROL_PLANE_TUNNEL_ID="tunnel_test123456789012345678901234" \
CONTROL_PLANE_API_KEY="test-control-plane-key" \
MEETILY_AUTOMATION_CLIENT_ID="test-client-id" \
TUNNEL_CLIENT_BIN="$fake_client" \
CAPTURE_ARGS="$capture" \
CAPTURE_ENV="$env_capture" \
  "$repo_root/scripts/meetily-chatgpt-tunnel.sh"

grep -Fxq 'run' "$capture"
grep -Fxq 'tunnel_test123456789012345678901234' "$capture"
grep -Fxq 'env:CONTROL_PLANE_API_KEY' "$capture"
grep -Fq "$repo_root/automation-client/dist/mcp.js" "$capture"
! grep -Fq 'test-control-plane-key' "$capture"
test -s "$env_capture"
printf 'ChatGPT tunnel launcher keeps the API key out of argv and forwards the Meetily MCP command.\n'
