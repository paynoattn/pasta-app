#!/bin/sh
# Cargo runner on macOS (see .cargo/config.toml). Before the app binary runs, signs it
# with the certificate named in PASTA_CODESIGN_IDENTITY, so the keychain recognises
# each rebuild as the same app and stops asking for the login password. Other binaries
# (tests, probes) and an unset variable run unchanged. See docs/development.md.
set -u

bin="$1"
if [ -n "${PASTA_CODESIGN_IDENTITY:-}" ] && [ "$(basename "$bin")" = "pasta-app" ]; then
  if ! codesign --force --sign "$PASTA_CODESIGN_IDENTITY" --identifier com.kingdomworks.pasta "$bin"; then
    echo "macos-dev-runner: couldn't sign with \"$PASTA_CODESIGN_IDENTITY\"; running unsigned" >&2
  fi
fi
exec "$@"
