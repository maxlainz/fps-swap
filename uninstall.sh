#!/bin/bash
# Removes what install.sh installed. Safe to run when nothing is installed.
set -euo pipefail

rm -f "$HOME/.local/bin/fps-swap"
rm -rf "$HOME/Library/Services/FPS Swap (24↔25).workflow"

if [ -x /System/Library/CoreServices/pbs ]; then
  /System/Library/CoreServices/pbs -update || true
fi

echo "Removed fps-swap and the FPS Swap (24↔25) Quick Action."
