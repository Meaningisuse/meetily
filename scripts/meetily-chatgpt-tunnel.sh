#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mcp_entry="$repo_root/automation-client/dist/mcp.js"
tunnel_client="${TUNNEL_CLIENT_BIN:-tunnel-client}"

if [[ -z "${CONTROL_PLANE_TUNNEL_ID:-}" ]]; then
  printf 'Set CONTROL_PLANE_TUNNEL_ID to the OpenAI Platform tunnel ID.\n' >&2
  exit 2
fi
if [[ -z "${MEETILY_AUTOMATION_CLIENT_ID:-}" ]]; then
  printf 'Set MEETILY_AUTOMATION_CLIENT_ID to the read-only client ID shown by Meetily.\n' >&2
  exit 2
fi
if [[ ! -f "$mcp_entry" ]]; then
  printf 'Meetily MCP client is not built; run pnpm --dir automation-client install --frozen-lockfile and pnpm --dir automation-client build.\n' >&2
  exit 2
fi
if [[ "$tunnel_client" == */* ]]; then
  if [[ ! -x "$tunnel_client" ]]; then
    printf 'tunnel-client is not executable: %s\n' "$tunnel_client" >&2
    exit 2
  fi
elif ! command -v "$tunnel_client" >/dev/null 2>&1; then
  printf 'Install the official OpenAI tunnel-client and make it available on PATH.\n' >&2
  exit 2
fi

if [[ -z "${CONTROL_PLANE_API_KEY:-}" ]]; then
  if [[ ! -r /dev/tty ]]; then
    printf 'Set CONTROL_PLANE_API_KEY in the process environment or run this script from a terminal to enter it securely.\n' >&2
    exit 2
  fi
  IFS= read -r -s -p 'OpenAI runtime API key: ' CONTROL_PLANE_API_KEY </dev/tty
  printf '\n' >/dev/tty
  if [[ -z "$CONTROL_PLANE_API_KEY" ]]; then
    printf 'An OpenAI runtime API key is required.\n' >&2
    exit 2
  fi
  export CONTROL_PLANE_API_KEY
fi

mcp_command="node $(printf '%q' "$mcp_entry")"
exec "$tunnel_client" run \
  --control-plane.tunnel-id "$CONTROL_PLANE_TUNNEL_ID" \
  --control-plane.api-key env:CONTROL_PLANE_API_KEY \
  --mcp.command "$mcp_command"
