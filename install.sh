#!/bin/sh
# Builds the server and installs it to ~/.local/bin. Restarts the server if
# it's running, so it uses the new build.
set -eu

cd "$(dirname "$0")"
cargo install --path . --root "$HOME/.local" --locked

executable="$HOME/.local/bin/claude-sdk-api-transcription-cleaner"
if "$executable" status > /dev/null; then
  "$executable" start
fi
