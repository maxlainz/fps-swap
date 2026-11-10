#!/bin/bash
# Installs fps-swap and its Finder Quick Action for the current user. No sudo.
# Works from an unzipped release (binary next to this script) or from the repo
# after `cargo build --release`. Safe to run again to upgrade.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
bin_dir="$HOME/.local/bin"
services_dir="$HOME/Library/Services"
wf_name="FPS Swap (24↔25).workflow"
wf_src="$here/quickaction/$wf_name"

if [ -f "$here/fps-swap" ]; then
  bin_src="$here/fps-swap"
elif [ -f "$here/target/release/fps-swap" ]; then
  bin_src="$here/target/release/fps-swap"
else
  echo "error: fps-swap binary not found next to install.sh or in target/release/ (run: cargo build --release)" >&2
  exit 1
fi
if [ ! -d "$wf_src" ]; then
  echo "error: Quick Action not found at $wf_src" >&2
  exit 1
fi

mkdir -p "$bin_dir" "$services_dir"
install -m 755 "$bin_src" "$bin_dir/fps-swap"
rm -rf "${services_dir:?}/$wf_name"
cp -R "$wf_src" "$services_dir/"

# Downloaded files carry quarantine; Gatekeeper would block the unsigned binary.
xattr -dr com.apple.quarantine "$bin_dir/fps-swap" "$services_dir/$wf_name" 2>/dev/null || true

if [ -x /System/Library/CoreServices/pbs ]; then
  /System/Library/CoreServices/pbs -update || true
fi

echo "Installed $bin_dir/fps-swap"
echo "Installed $services_dir/$wf_name"
echo "Finder: right-click movie files > Quick Actions > FPS Swap (24↔25)"

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) echo "warning: $bin_dir is not in your PATH; add it to use fps-swap from the terminal:" >&2
     echo "  echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.zshrc" >&2 ;;
esac
