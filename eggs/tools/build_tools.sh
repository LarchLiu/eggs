#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"

dest_dir="$HOME/.eggs/bin"
cache_dir="$repo_root/.swift-module-cache"

usage() {
  cat <<'EOF'
Usage:
  eggs/tools/build_tools.sh [--dest <dir>] [--cache-dir <dir>] [--with-helpers]

Compiles the main eggs Swift tools into the target bin directory.

Options:
  --dest <dir>       Output directory for compiled tools. Default: ~/.eggs/bin
  --cache-dir <dir>  Swift module cache directory. Default: <repo>/.swift-module-cache
  --with-helpers     Also compile helper tools: bounds_sprite and check_sprite
  --help             Show this help.
EOF
}

with_helpers=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dest)
      [[ $# -ge 2 ]] || { echo "Error: --dest requires a value." >&2; exit 1; }
      dest_dir="$2"
      shift 2
      ;;
    --cache-dir)
      [[ $# -ge 2 ]] || { echo "Error: --cache-dir requires a value." >&2; exit 1; }
      cache_dir="$2"
      shift 2
      ;;
    --with-helpers)
      with_helpers=1
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Error: unknown option '$1'." >&2
      usage >&2
      exit 1
      ;;
  esac
done

mkdir -p "$dest_dir" "$cache_dir"

tools=(
  extract_sprite
  merge_spritesheets
  resize_crop_frames
)

if [[ "$with_helpers" -eq 1 ]]; then
  tools+=(
    bounds_sprite
    check_sprite
  )
fi

for tool in "${tools[@]}"; do
  src="$script_dir/${tool}.swift"
  out="$dest_dir/$tool"
  echo "Compiling $tool -> $out"
  CLANG_MODULE_CACHE_PATH="$cache_dir" \
    swiftc -module-cache-path "$cache_dir" "$src" -o "$out"
done

echo "Built ${#tools[@]} tools into $dest_dir"
