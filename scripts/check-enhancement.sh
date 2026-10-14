#!/usr/bin/env bash
set -euo pipefail

stage=${1:---all}
if [ "$#" -gt 1 ]; then
  echo "Usage: $0 [--frontend|--native|--bundle|--all]" >&2
  exit 2
fi
case "$stage" in
  --frontend|--native|--bundle|--all) ;;
  *) echo "Unknown check stage: $stage" >&2; exit 2 ;;
esac

repo_root=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo_root"

frontend_checks() {
  pnpm -C frontend install --frozen-lockfile
  node --test .github/verify-portable-ggml.test.cjs scripts/check-enhancement.test.cjs
  bun test frontend/tests
  pnpm -C frontend exec tsc --noEmit
  pnpm -C frontend build
}

prepare_helper() {
  local profile=$1
  local host
  host=$(rustc -vV | sed -n 's/^host: //p')
  local args=(build --locked -p llama-helper)
  if [ "$profile" = release ]; then args+=(--release); fi
  case "$host" in *apple-darwin) args+=(--features metal) ;; esac
  cargo "${args[@]}"
  mkdir -p frontend/src-tauri/binaries
  cp "target/$profile/llama-helper" "frontend/src-tauri/binaries/llama-helper-$host"
}

prepare_native_environment() {
  # cidre requires full Xcode. Keep the user's global xcode-select untouched.
  if [ "$(uname -s)" = Darwin ] && ! xcodebuild -version >/dev/null 2>&1; then
    if [ -d /Applications/Xcode.app/Contents/Developer ]; then
      export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
    else
      echo "Full Xcode is required; set DEVELOPER_DIR to its Contents/Developer directory." >&2
      return 1
    fi
  fi
  cmake --version
}

native_checks() {
  prepare_native_environment
  cargo metadata --locked --no-deps --format-version 1 >/dev/null
  prepare_helper debug
  cargo test --locked -p meetily --lib
}

case "$stage" in
  --frontend) frontend_checks ;;
  --native) native_checks ;;
  --bundle)
    prepare_native_environment
    prepare_helper release
    # Local validation packages do not require the production updater key.
    pnpm -C frontend exec tauri build --ci --bundles app --config '{"bundle":{"createUpdaterArtifacts":false}}' -- --locked
    ;;
  --all) frontend_checks; native_checks ;;
esac
