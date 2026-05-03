#!/usr/bin/env bash
set -euo pipefail

REPO_URL="${KCODE_REPO_URL:-https://github.com/icedmoca/kcode.git}"
HF_REPO="${KCODE_HF_REPO:-icedmoca/kcode-oss-20b-mxfp4}"
MODEL_FILE="${KCODE_MODEL_FILE:-kcode-oss-20b-mxfp4.gguf}"
KCODE_HOME="${KCODE_HOME:-$HOME/.kcode}"
SRC_DIR="$KCODE_HOME/build-src/kcode"
MODEL_DIR="$KCODE_HOME/models/gguf"
BIN_DIR="${KCODE_BIN_DIR:-$HOME/.local/bin}"
BUILD_PROFILE="${KCODE_BUILD_PROFILE:-release}"
SKIP_MODEL="${KCODE_SKIP_MODEL:-0}"
SKIP_CHROMIUM_MCP="${KCODE_SKIP_CHROMIUM_MCP:-0}"
HF_BASE="https://huggingface.co/$HF_REPO/resolve/main"
MODEL_PATH="$MODEL_DIR/$MODEL_FILE"
LOG_DIR="$KCODE_HOME/logs"
INSTALL_LOG="$LOG_DIR/install-$(date +%Y%m%d-%H%M%S).log"
PRETTY_INSTALL="${KCODE_PRETTY_INSTALL:-1}"

color() { printf '\033[%sm%s\033[0m' "$1" "$2"; }
log() { printf '\033[1;32m[kcode install]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[kcode warn]\033[0m %s\n' "$*" >&2; }
err() { printf '\033[1;31m[kcode error]\033[0m %s\n' "$*" >&2; }

term_width() {
  local cols
  cols=$(tput cols 2>/dev/null || printf '80')
  [ "$cols" -gt 0 ] 2>/dev/null || cols=80
  printf '%s' "$cols"
}

center() {
  local text="$1" width pad
  width=$(term_width)
  # Strip ANSI escapes approximately for padding decisions.
  local plain="${text//$'\033'\[[0-9;]*m/}"
  if [ "${#plain}" -ge "$width" ]; then
    printf '%s\n' "$text"
  else
    pad=$(( (width - ${#plain}) / 2 ))
    printf '%*s%s\n' "$pad" '' "$text"
  fi
}

banner() {
  if [ "$PRETTY_INSTALL" = "0" ]; then
    log "Kcode installer"
    return
  fi
  printf '\n'
  center "$(color '1;36' '+----------------------------------------------+')"
  center "$(color '1;36' '|')  $(color '1;37' 'Kcode Installer')  $(color '2;37' 'quiet, fast, grounded')  $(color '1;36' '|')"
  center "$(color '1;36' '+----------------------------------------------+')"
  printf '\n'
  center "$(color '2;37' "full logs: $INSTALL_LOG")"
  printf '\n'
}

progress_bar() {
  local percent="$1" width=28 filled empty
  filled=$(( percent * width / 100 ))
  empty=$(( width - filled ))
  printf '['
  printf '%*s' "$filled" '' | tr ' ' '#'
  printf '%*s' "$empty" '' | tr ' ' '-'
  printf '] %3d%%' "$percent"
}

pretty_step() {
  local percent="$1" title="$2" detail="$3"
  shift 3
  local spin='|/-\\'
  local tmp status pid frame=0
  tmp=$(mktemp)
  mkdir -p "$LOG_DIR"
  {
    printf '\n[%s] %s - %s\n' "$(date -Is)" "$title" "$detail"
    "$@"
  } >>"$INSTALL_LOG" 2>&1 &
  pid=$!

  if [ "$PRETTY_INSTALL" = "0" ] || [ ! -t 1 ]; then
    printf '[kcode install] %s... ' "$title"
    if wait "$pid"; then
      printf 'ok\n'
      rm -f "$tmp"
      return 0
    else
      status=$?
      printf 'failed\n'
      err "$title failed. See $INSTALL_LOG"
      rm -f "$tmp"
      return "$status"
    fi
  fi

  tput civis 2>/dev/null || true
  while kill -0 "$pid" 2>/dev/null; do
    local ch="${spin:$((frame % ${#spin})):1}"
    printf '\r\033[K'
    center "$(color '1;36' "$ch") $(color '1;37' "$title")  $(color '2;37' "$detail")  $(color '1;32' "$(progress_bar "$percent")")" | tr -d '\n'
    frame=$((frame + 1))
    sleep 0.12
  done

  if wait "$pid"; then
    printf '\r\033[K'
    center "$(color '1;32' '[ok]') $(color '1;37' "$title")  $(color '2;37' "$detail")  $(color '1;32' "$(progress_bar "$percent")")" | tr -d '\n'
    printf '\n'
    tput cnorm 2>/dev/null || true
    rm -f "$tmp"
    return 0
  else
    status=$?
    printf '\r\033[K'
    center "$(color '1;31' '[x]') $(color '1;37' "$title")  $(color '2;37' 'failed')"
    tput cnorm 2>/dev/null || true
    err "$title failed. Full output saved to $INSTALL_LOG"
    tail -n 20 "$INSTALL_LOG" >&2 || true
    rm -f "$tmp"
    return "$status"
  fi
}

need_cmd() { command -v "$1" >/dev/null 2>&1; }

install_deps() {
  if need_cmd git && need_cmd curl && need_cmd cargo; then
    return 0
  fi

  if [ "$(uname -s)" = "Darwin" ]; then
    if ! need_cmd brew; then
      warn "Homebrew is required on macOS. Install it from https://brew.sh and rerun this installer."
      exit 1
    fi
    brew install git curl rust
  elif need_cmd apt-get; then
    sudo apt-get update
    sudo apt-get install -y git curl build-essential pkg-config libssl-dev
    if ! need_cmd cargo; then
      curl https://sh.rustup.rs -sSf | sh -s -- -y
      # shellcheck disable=SC1090
      source "$HOME/.cargo/env"
    fi
  elif need_cmd dnf; then
    sudo dnf install -y git curl gcc gcc-c++ openssl-devel pkg-config rust cargo
  elif need_cmd pacman; then
    sudo pacman -S --needed git curl base-devel openssl pkgconf rust
  else
    warn "Could not detect a supported package manager. Please install git, curl, and Rust, then rerun."
    exit 1
  fi
}

fetch_source() {
  if [ -d "$SRC_DIR/.git" ]; then
    git -C "$SRC_DIR" fetch --depth=1 origin main || git -C "$SRC_DIR" fetch origin
    git -C "$SRC_DIR" checkout main || true
    git -C "$SRC_DIR" pull --ff-only || true
  else
    rm -rf "$SRC_DIR"
    mkdir -p "$(dirname "$SRC_DIR")"
    git clone --depth=1 "$REPO_URL" "$SRC_DIR"
  fi
}

download_model() {
  if [ "$SKIP_MODEL" = "1" ]; then
    return 0
  fi
  mkdir -p "$MODEL_DIR"
  if [ ! -s "$MODEL_PATH" ]; then
    local url tmp
    url="$HF_BASE/$MODEL_FILE"
    tmp="$MODEL_PATH.part"
    curl -L --fail --retry 5 --retry-delay 5 --continue-at - -o "$tmp" "$url"
    mv -f "$tmp" "$MODEL_PATH"
  fi
  ln -sfn "$MODEL_FILE" "$MODEL_DIR/kcode-oss-20b-mxfp4"
  ln -sfn "$MODEL_FILE" "$MODEL_DIR/gpt-oss-20b-mxfp4_moe.gguf"
  ln -sfn "$MODEL_FILE" "$MODEL_DIR/jcode-gpt-oss-20b.gguf"
}

build_kcode() {
  if [ "$BUILD_PROFILE" = "debug" ]; then
    cargo build --manifest-path "$SRC_DIR/Cargo.toml" --bin kcode
  else
    cargo build --manifest-path "$SRC_DIR/Cargo.toml" --release --bin kcode
  fi
}

install_binary() {
  local target_dir dest version
  if [ "$BUILD_PROFILE" = "debug" ]; then
    target_dir="$SRC_DIR/target/debug"
  else
    target_dir="$SRC_DIR/target/release"
  fi
  version="$($target_dir/kcode --version 2>/dev/null | awk '{print $2}')"
  version="${version:-dev}"
  dest="$KCODE_HOME/builds/versions/$version"
  mkdir -p "$dest" "$KCODE_HOME/builds/stable"
  cp "$target_dir/kcode" "$dest/kcode"
  chmod +x "$dest/kcode"
  cp "$dest/kcode" "$dest/jcode"
  chmod +x "$dest/jcode"
  cp "$dest/kcode" "$KCODE_HOME/builds/stable/kcode.new"
  cp "$dest/jcode" "$KCODE_HOME/builds/stable/jcode.new"
  mv -f "$KCODE_HOME/builds/stable/kcode.new" "$KCODE_HOME/builds/stable/kcode"
  mv -f "$KCODE_HOME/builds/stable/jcode.new" "$KCODE_HOME/builds/stable/jcode"
  ln -sfn "versions/$version" "$KCODE_HOME/builds/current"
}

install_chromium_bridge() {
  if [ "$SKIP_CHROMIUM_MCP" = "1" ]; then
    return 0
  fi
  local bridge_dir config_dir config_file bridge_mcp
  bridge_dir="$KCODE_HOME/vendor/chromium-agent-bridge"
  config_dir="$KCODE_HOME/mcp"
  config_file="$config_dir/mcp.json"
  bridge_mcp="$bridge_dir/chromium-agent-bridge-mcp"

  if [ -d "$SRC_DIR/vendor/chromium-agent-bridge" ]; then
    rm -rf "$bridge_dir.tmp"
    mkdir -p "$bridge_dir.tmp"
    cp -R "$SRC_DIR/vendor/chromium-agent-bridge/." "$bridge_dir.tmp/"
    chmod +x "$bridge_dir.tmp/chromium-agent-bridge" "$bridge_dir.tmp/chromium-agent-bridge-mcp"
    rm -rf "$bridge_dir"
    mv "$bridge_dir.tmp" "$bridge_dir"
  fi

  mkdir -p "$config_dir"
  CONFIG_FILE="$config_file" BRIDGE_MCP="$bridge_mcp" python3 - <<'PY'
import json
import os
from pathlib import Path
path = Path(os.environ['CONFIG_FILE'])
bridge_mcp = os.environ['BRIDGE_MCP']
if path.exists():
    try:
        data = json.loads(path.read_text())
    except Exception:
        backup = path.with_suffix(path.suffix + '.bak')
        backup.write_text(path.read_text())
        data = {}
else:
    data = {}
servers = data.setdefault('servers', {})
servers['chromium-agent-bridge'] = {
    'command': bridge_mcp,
    'args': [],
    'env': {},
    'shared': True,
}
path.write_text(json.dumps(data, indent=2) + '\n')
PY
}

write_launchers() {
  cat > "$BIN_DIR/kcode" <<EOF
#!/usr/bin/env bash
export KCODE_HOME="\${KCODE_HOME:-$KCODE_HOME}"
exec "$KCODE_HOME/builds/current/kcode" "\$@"
EOF
  chmod +x "$BIN_DIR/kcode"
  cat > "$BIN_DIR/jcode" <<EOF
#!/usr/bin/env bash
export KCODE_HOME="\${KCODE_HOME:-$KCODE_HOME}"
exec "$KCODE_HOME/builds/current/kcode" "\$@"
EOF
  chmod +x "$BIN_DIR/jcode"
}

main() {
  mkdir -p "$KCODE_HOME" "$MODEL_DIR" "$BIN_DIR" "$LOG_DIR"
  banner
  pretty_step 10 "Preparing system" "checking tools and dependencies" install_deps
  pretty_step 25 "Fetching source" "syncing $REPO_URL" fetch_source
  pretty_step 45 "Preparing model" "$MODEL_FILE" download_model
  pretty_step 70 "Building Kcode" "$BUILD_PROFILE profile" build_kcode
  pretty_step 84 "Installing binary" "$BIN_DIR/kcode" install_binary
  pretty_step 92 "Browser bridge" "registering Chromium MCP helper" install_chromium_bridge
  pretty_step 98 "Launchers" "writing kcode and jcode commands" write_launchers

  printf '\n'
  center "$(color '1;32' '[ok] Kcode is ready')"
  center "$(color '2;37' "version: $($BIN_DIR/kcode --version 2>/dev/null || true)")"
  center "$(color '2;37' "binary:  $BIN_DIR/kcode")"
  center "$(color '2;37' "home:    $KCODE_HOME")"
  center "$(color '2;37' "model:   $MODEL_PATH")"
  printf '\n'
  if [ "$SKIP_CHROMIUM_MCP" != "1" ]; then
    warn "Chrome requires one manual step: load unpacked extension from $KCODE_HOME/vendor/chromium-agent-bridge/extension"
  fi
  case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) warn "$BIN_DIR is not on PATH. Add: export PATH=\"$BIN_DIR:\$PATH\"" ;;
  esac
}

main "$@"
