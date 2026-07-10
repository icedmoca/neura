#!/usr/bin/env bash
set -euo pipefail

REPO_URL="${NEURA_REPO_URL:-https://github.com/icedmoca/neura.git}"
HF_REPO="${NEURA_HF_REPO:-icedmoca/neura-oss-20b-mxfp4}"
MODEL_FILE="${NEURA_MODEL_FILE:-neura-oss-20b-mxfp4.gguf}"
NEURA_HOME="${NEURA_HOME:-$HOME/.neura}"
SRC_DIR="$NEURA_HOME/build-src/neura"
MODEL_DIR="$NEURA_HOME/models/gguf"
BIN_DIR="${NEURA_BIN_DIR:-$HOME/.local/bin}"
BUILD_PROFILE="${NEURA_BUILD_PROFILE:-release}"
SKIP_MODEL="${NEURA_SKIP_MODEL:-0}"
SKIP_CHROMIUM_MCP="${NEURA_SKIP_CHROMIUM_MCP:-0}"
HF_BASE="https://huggingface.co/$HF_REPO/resolve/main"
MODEL_PATH="$MODEL_DIR/$MODEL_FILE"
LOG_DIR="$NEURA_HOME/logs"
INSTALL_LOG="$LOG_DIR/install-$(date +%Y%m%d-%H%M%S).log"
PRETTY_INSTALL="${NEURA_PRETTY_INSTALL:-1}"

C_RESET='\033[0m'; C_DIM='\033[2;37m'; C_BOLD='\033[1m'; C_GREEN='\033[1;32m'; C_BLUE='\033[1;34m'; C_CYAN='\033[1;36m'; C_YELLOW='\033[1;33m'; C_RED='\033[1;31m'
color() { printf '%b%s%b' "$1" "$2" "$C_RESET"; }
warn() { printf '%b%s%b %s\n' "$C_YELLOW" 'warn' "$C_RESET" "$*" >&2; }
err() { printf '%b%s%b %s\n' "$C_RED" 'error' "$C_RESET" "$*" >&2; }

bar() {
  local pct="$1" width=34 filled empty
  [ "$pct" -lt 0 ] 2>/dev/null && pct=0
  [ "$pct" -gt 100 ] 2>/dev/null && pct=100
  filled=$(( pct * width / 100 )); empty=$(( width - filled ))
  printf '['; printf '%*s' "$filled" '' | tr ' ' '#'; printf '%*s' "$empty" '' | tr ' ' '-'; printf '] %3d%%' "$pct"
}

header() {
  printf '\n%b' "$C_CYAN"
  cat <<'ART'
__/\\\\\_____/\\\__/\\\\\\\\\\\\\\\__/\\\________/\\\____/\\\\\\\\\_________/\\\\\\\\\____
 _\/\\\\\\___\/\\\_\/\\\///////////__\/\\\_______\/\\\__/\\\///////\\\_____/\\\\\\\\\\\\\__
  _\/\\\/\\\__\/\\\_\/\\\_____________\/\\\_______\/\\\_\/\\\_____\/\\\____/\\\/////////\\\_
   _\/\\\//\\\_\/\\\_\/\\\\\\\\\\\_____\/\\\_______\/\\\_\/\\\\\\\\\\\/____\/\\\_______\/\\\_
    _\/\\\\//\\\\/\\\_\/\\\///////______\/\\\_______\/\\\_\/\\\//////\\\____\/\\\\\\\\\\\\\\\_
     _\/\\\_\//\\\/\\\_\/\\\_____________\/\\\_______\/\\\_\/\\\____\//\\\___\/\\\/////////\\\_
      _\/\\\__\//\\\\\\_\/\\\_____________\//\\\______/\\\__\/\\\_____\//\\\__\/\\\_______\/\\\_
       _\/\\\___\//\\\\\_\/\\\\\\\\\\\\\\\__\///\\\\\\\\\/___\/\\\______\//\\\_\/\\\_______\/\\\_
        _\///_____\/////__\///////////////_____\/////////_____\///________\///__\///________\///__
ART
  printf '%b' "$C_RESET"
  printf '%b%s%b\n' "$C_DIM" 'Modern local AI coding agent installer' "$C_RESET"
  printf '%b%s%b %s\n\n' "$C_DIM" 'Full install log:' "$C_RESET" "$INSTALL_LOG"
}

set_status() {
  local label="$1" pct="$2" msg="$3"
  if [ "$PRETTY_INSTALL" = "0" ] || [ ! -t 1 ]; then
    printf '  %-18s %s %s\n' "$label" "$(bar "$pct")" "$msg"
  else
    printf '\r\033[K  %-18s %s %s' "$label" "$(bar "$pct")" "$msg"
  fi
}

finish_status() {
  local label="$1" msg="$2"
  if [ "$PRETTY_INSTALL" != "0" ] && [ -t 1 ]; then printf '\r\033[K'; fi
  printf '  %b%-18s%b %s %s\n' "$C_GREEN" '[ok]' "$C_RESET" "$label" "$msg"
}

fail_status() {
  local label="$1" msg="$2"
  if [ "$PRETTY_INSTALL" != "0" ] && [ -t 1 ]; then printf '\r\033[K'; fi
  printf '  %b%-18s%b %s %s\n' "$C_RED" '[x]' "$C_RESET" "$label" "$msg" >&2
}

run_logged() {
  local label="$1" message="$2"; shift 2
  printf '\n[%s] %s - %s\n' "$(date -Is)" "$label" "$message" >>"$INSTALL_LOG"
  "$@" >>"$INSTALL_LOG" 2>&1
}

animate_command() {
  local label="$1" message="$2" start_pct="$3" end_pct="$4"; shift 4
  local pid status pct spin='|/-\' frame=0 hold=0
  [ "$end_pct" -gt 99 ] 2>/dev/null && end_pct=99
  [ "$start_pct" -ge "$end_pct" ] 2>/dev/null && start_pct=$((end_pct - 1))
  pct=$start_pct
  printf '\n[%s] %s - %s\n' "$(date -Is)" "$label" "$message" >>"$INSTALL_LOG"
  "$@" >>"$INSTALL_LOG" 2>&1 & pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$pct" -lt "$end_pct" ]; then
      set_status "$label" "$pct" "${spin:$((frame % 4)):1} $message"
      frame=$((frame + 1))
      # Monotonic easing: fast at first, then slower near the end so the bar
      # never jumps backward or sits at 100 while the command is still running.
      if [ "$pct" -lt 55 ] || [ $((frame % 2)) -eq 0 ]; then
        pct=$((pct + 1))
      fi
    else
      set_status "$label" "$end_pct" "${spin:$((frame % 4)):1} finishing..."
      frame=$((frame + 1))
      hold=$((hold + 1))
    fi
    sleep 0.15
  done
  if wait "$pid"; then
    finish_status "$label" "$(bar 100) done"
  else
    status=$?
    fail_status "$label" "failed - see $INSTALL_LOG"
    tail -n 25 "$INSTALL_LOG" >&2 || true
    return "$status"
  fi
}

run_activity() {
  local label="$1" message="$2"; shift 2
  local pid status spin='|/-\' frame=0 started elapsed
  started=$(date +%s)
  printf '\n[%s] %s - %s\n' "$(date -Is)" "$label" "$message" >>"$INSTALL_LOG"
  "$@" >>"$INSTALL_LOG" 2>&1 & pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    elapsed=$(( $(date +%s) - started ))
    if [ "$PRETTY_INSTALL" = "0" ] || [ ! -t 1 ]; then
      # Non-TTY logs get occasional heartbeat lines instead of thousands of redraws.
      if [ $((frame % 20)) -eq 0 ]; then
        printf '  %-18s %s %s elapsed %ss\n' "$label" "${spin:$((frame % 4)):1}" "$message" "$elapsed"
      fi
    else
      printf '\r\033[K  %-18s %s %s elapsed %ss' "$label" "${spin:$((frame % 4)):1}" "$message" "$elapsed"
    fi
    frame=$((frame + 1))
    sleep 0.25
  done
  if wait "$pid"; then
    if [ "$PRETTY_INSTALL" != "0" ] && [ -t 1 ]; then printf '\r\033[K'; fi
    finish_status "$label" "$(bar 100) done in $(( $(date +%s) - started ))s"
  else
    status=$?
    fail_status "$label" "failed - see $INSTALL_LOG"
    tail -n 25 "$INSTALL_LOG" >&2 || true
    return "$status"
  fi
}

need_cmd() { command -v "$1" >/dev/null 2>&1; }
check_line() {
  local name="$1" status="$2" detail="$3"
  case "$status" in
    ok) printf '  %b%-18s%b %s\n' "$C_GREEN" '[ok]' "$C_RESET" "$name - $detail" ;;
    skip) printf '  %b%-18s%b %s\n' "$C_DIM" '[skip]' "$C_RESET" "$name - $detail" ;;
    need) printf '  %b%-18s%b %s\n' "$C_YELLOW" '[need]' "$C_RESET" "$name - $detail" ;;
  esac
}

install_deps() {
  printf '%b%s%b\n' "$C_BOLD" 'System checks' "$C_RESET"
  need_cmd git && check_line git ok 'already installed' || check_line git need 'will install'
  need_cmd curl && check_line curl ok 'already installed' || check_line curl need 'will install'
  need_cmd cargo && check_line rust ok 'already installed' || check_line rust need 'will install'
  if need_cmd git && need_cmd curl && need_cmd cargo; then
    finish_status 'system' "$(bar 100) all tools ready"
    return 0
  fi
  if [ "$(uname -s)" = "Darwin" ]; then
    need_cmd brew || { err 'Homebrew is required on macOS: https://brew.sh'; exit 1; }
    animate_command 'packages' 'installing git, curl, and rust with Homebrew' 5 95 brew install git curl rust
  elif need_cmd apt-get; then
    animate_command 'packages' 'refreshing package index' 5 45 sudo apt-get update
    animate_command 'packages' 'installing build dependencies' 45 85 sudo apt-get install -y git curl build-essential pkg-config libssl-dev
    if ! need_cmd cargo; then
      animate_command 'rust' 'installing Rust toolchain' 5 95 bash -c 'curl https://sh.rustup.rs -sSf | sh -s -- -y'
      # shellcheck disable=SC1090
      source "$HOME/.cargo/env"
    fi
  elif need_cmd dnf; then
    animate_command 'packages' 'installing build dependencies with dnf' 5 95 sudo dnf install -y git curl gcc gcc-c++ openssl-devel pkg-config rust cargo
  elif need_cmd pacman; then
    animate_command 'packages' 'installing build dependencies with pacman' 5 95 sudo pacman -S --needed git curl base-devel openssl pkgconf rust
  else
    err 'Could not detect a supported package manager. Install git, curl, and Rust, then rerun.'
    exit 1
  fi
  finish_status 'system' "$(bar 100) all tools ready"
}

reset_managed_checkout() {
  if [ ! -d "$SRC_DIR/.git" ]; then
    return 1
  fi
  git -C "$SRC_DIR" remote set-url origin "$REPO_URL" >/dev/null 2>&1 || true
  git -C "$SRC_DIR" fetch --depth=1 origin main
  git -C "$SRC_DIR" checkout -B main FETCH_HEAD
  git -C "$SRC_DIR" reset --hard FETCH_HEAD
  git -C "$SRC_DIR" clean -fdx
}

clone_managed_checkout() {
  rm -rf "$SRC_DIR"
  mkdir -p "$(dirname "$SRC_DIR")"
  git clone --depth=1 --branch main "$REPO_URL" "$SRC_DIR"
}

fetch_source() {
  printf '\n%b%s%b\n' "$C_BOLD" 'Updating Neura' "$C_RESET"
  if [ -d "$SRC_DIR/.git" ]; then
    check_line update ok 'installed copy found; checking for updates'
    if animate_command 'update' 'syncing latest Neura release' 5 98 reset_managed_checkout; then
      finish_status 'update' "$(bar 100) latest version ready"
      return 0
    fi
    local backup
    backup="$SRC_DIR.backup.$(date +%Y%m%d%H%M%S)"
    warn "Local installer cache could not be updated cleanly; backing it up and downloading a fresh copy."
    mv "$SRC_DIR" "$backup"
  else
    check_line update need 'downloading Neura'
  fi
  animate_command 'update' 'downloading latest Neura release' 5 98 clone_managed_checkout
  finish_status 'update' "$(bar 100) latest version ready"
}

download_model() {
  printf '\n%b%s%b\n' "$C_BOLD" 'AI model' "$C_RESET"
  if [ "$SKIP_MODEL" = "1" ]; then
    check_line model skip 'NEURA_SKIP_MODEL=1'
    finish_status 'model' "$(bar 100) skipped"
    return 0
  fi
  mkdir -p "$MODEL_DIR"
  if [ -s "$MODEL_PATH" ]; then
    check_line model ok "already present: $MODEL_PATH"
    finish_status 'model' "$(bar 100) ready"
  else
    check_line model need "downloading $MODEL_FILE"
    local url tmp pct
    url="$HF_BASE/$MODEL_FILE"; tmp="$MODEL_PATH.part"
    printf '\n[%s] model download - %s\n' "$(date -Is)" "$url" >>"$INSTALL_LOG"
    if [ "$PRETTY_INSTALL" = "0" ] || [ ! -t 1 ]; then
      curl -L --fail --retry 5 --retry-delay 5 --continue-at - --progress-bar -o "$tmp" "$url" 2>>"$INSTALL_LOG"
    else
      curl -L --fail --retry 5 --retry-delay 5 --continue-at - --progress-bar -o "$tmp" "$url" 2>&1 \
        | while IFS= read -r line; do
            pct=$(printf '%s' "$line" | grep -oE '[0-9]{1,3}(\.[0-9]+)?%' | tail -1 | tr -d '%' | cut -d. -f1 || true)
            [ -n "${pct:-}" ] || pct=0
            set_status 'model' "$pct" "downloading $MODEL_FILE"
          done
    fi
    mv -f "$tmp" "$MODEL_PATH"
    finish_status 'model' "$(bar 100) downloaded"
  fi
  ln -sfn "$MODEL_FILE" "$MODEL_DIR/neura-oss-20b-mxfp4"
  ln -sfn "$MODEL_FILE" "$MODEL_DIR/gpt-oss-20b-mxfp4_moe.gguf"
  ln -sfn "$MODEL_FILE" "$MODEL_DIR/jcode-gpt-oss-20b.gguf"
}

build_neura() {
  printf '\n%b%s%b\n' "$C_BOLD" 'Build' "$C_RESET"
  check_line profile ok "$BUILD_PROFILE"
  if [ "$BUILD_PROFILE" = "debug" ]; then
    run_activity 'build' 'compiling Neura debug binary' cargo build --manifest-path "$SRC_DIR/Cargo.toml" --bin neura
  else
    run_activity 'build' 'compiling optimized Neura binary' cargo build --manifest-path "$SRC_DIR/Cargo.toml" --release --bin neura
  fi
}

install_binary() {
  printf '\n%b%s%b\n' "$C_BOLD" 'Install' "$C_RESET"
  local target_dir dest version
  [ "$BUILD_PROFILE" = "debug" ] && target_dir="$SRC_DIR/target/debug" || target_dir="$SRC_DIR/target/release"
  version="$($target_dir/neura --version 2>/dev/null | awk '{print $2}')"; version="${version:-dev}"
  dest="$NEURA_HOME/builds/versions/$version"
  run_logged install 'copying versioned binaries' mkdir -p "$dest" "$NEURA_HOME/builds/stable"
  cp "$target_dir/neura" "$dest/neura"; chmod +x "$dest/neura"
  cp "$dest/neura" "$dest/jcode"; chmod +x "$dest/jcode"
  cp "$dest/neura" "$NEURA_HOME/builds/stable/neura.new"; cp "$dest/jcode" "$NEURA_HOME/builds/stable/jcode.new"
  mv -f "$NEURA_HOME/builds/stable/neura.new" "$NEURA_HOME/builds/stable/neura"
  mv -f "$NEURA_HOME/builds/stable/jcode.new" "$NEURA_HOME/builds/stable/jcode"
  ln -sfn "versions/$version" "$NEURA_HOME/builds/current"
  finish_status 'binary' "$(bar 100) installed version $version"
}

install_chromium_bridge() {
  printf '\n%b%s%b\n' "$C_BOLD" 'Browser integration' "$C_RESET"
  if [ "$SKIP_CHROMIUM_MCP" = "1" ]; then
    check_line bridge skip 'NEURA_SKIP_CHROMIUM_MCP=1'
    finish_status 'bridge' "$(bar 100) skipped"
    return 0
  fi
  local bridge_dir config_dir config_file bridge_mcp
  bridge_dir="$NEURA_HOME/vendor/chromium-agent-bridge"; config_dir="$NEURA_HOME/mcp"; config_file="$config_dir/mcp.json"; bridge_mcp="$bridge_dir/chromium-agent-bridge-mcp"
  if [ -d "$SRC_DIR/vendor/chromium-agent-bridge" ]; then
    animate_command 'bridge' 'installing Chromium helper files' 5 80 bash -c 'rm -rf "$0.tmp" && mkdir -p "$0.tmp" && cp -R "$1/vendor/chromium-agent-bridge/." "$0.tmp/" && chmod +x "$0.tmp/chromium-agent-bridge" "$0.tmp/chromium-agent-bridge-mcp" && rm -rf "$0" && mv "$0.tmp" "$0"' "$bridge_dir" "$SRC_DIR"
  else
    check_line bridge skip 'helper files not included in this checkout'
  fi
  mkdir -p "$config_dir"
  CONFIG_FILE="$config_file" BRIDGE_MCP="$bridge_mcp" python3 - <<'PYCFG'
import json, os
from pathlib import Path
path=Path(os.environ['CONFIG_FILE']); bridge=os.environ['BRIDGE_MCP']
if path.exists():
    try: data=json.loads(path.read_text())
    except Exception:
        path.with_suffix(path.suffix+'.bak').write_text(path.read_text()); data={}
else: data={}
data.setdefault('servers', {})['chromium-agent-bridge']={'command':bridge,'args':[],'env':{},'shared':True}
path.write_text(json.dumps(data, indent=2)+'\n')
PYCFG
  finish_status 'bridge' "$(bar 100) configured"
}

write_launchers() {
  printf '\n%b%s%b\n' "$C_BOLD" 'Commands' "$C_RESET"
  mkdir -p "$BIN_DIR"
  cat > "$BIN_DIR/neura" <<'LAUNCHER'
#!/usr/bin/env bash
export NEURA_HOME="${NEURA_HOME:-__NEURA_HOME__}"
# Phase D cognitive-state trigger: ON by default (override by exporting these
# before launch). Fail-quiet if the fusion service (:8801) is down.
export NEURA_COGNITION_TRIGGERS="${NEURA_COGNITION_TRIGGERS:-1}"
export NEURA_COGNITION_MIN_LEVEL="${NEURA_COGNITION_MIN_LEVEL:-warn}"
export NEURA_LOGITLENS_URL="${NEURA_LOGITLENS_URL:-http://127.0.0.1:8801}"
export NEURA_COGNITION_TIMEOUT_MS="${NEURA_COGNITION_TIMEOUT_MS:-8000}"
exec "$NEURA_HOME/builds/current/neura" "$@"
LAUNCHER
  sed -i.bak "s#__NEURA_HOME__#$NEURA_HOME#g" "$BIN_DIR/neura" && rm -f "$BIN_DIR/neura.bak"
  chmod +x "$BIN_DIR/neura"
  cat > "$BIN_DIR/jcode" <<'LAUNCHER'
#!/usr/bin/env bash
export NEURA_HOME="${NEURA_HOME:-__NEURA_HOME__}"
exec "$NEURA_HOME/builds/current/neura" "$@"
LAUNCHER
  sed -i.bak "s#__NEURA_HOME__#$NEURA_HOME#g" "$BIN_DIR/jcode" && rm -f "$BIN_DIR/jcode.bak"
  chmod +x "$BIN_DIR/jcode"
  finish_status 'commands' "$(bar 100) neura and jcode are ready"
}

main() {
  mkdir -p "$NEURA_HOME" "$MODEL_DIR" "$BIN_DIR" "$LOG_DIR"
  header
  install_deps
  fetch_source
  download_model
  build_neura
  install_binary
  install_chromium_bridge
  write_launchers
  printf '\n%b%s%b\n' "$C_GREEN" 'Neura installed successfully.' "$C_RESET"
  printf '  Version: %s\n' "$($BIN_DIR/neura --version 2>/dev/null || true)"
  printf '  Binary:  %s\n' "$BIN_DIR/neura"
  printf '  Home:    %s\n' "$NEURA_HOME"
  printf '  Logs:    %s\n' "$INSTALL_LOG"
  printf '\n%bRun '\''neura'\'' to start using Neura.%b\n' "$C_BOLD" "$C_RESET"
  if [ "$SKIP_CHROMIUM_MCP" != "1" ]; then
    warn "Chrome optional step: load unpacked extension from $NEURA_HOME/vendor/chromium-agent-bridge/extension"
  fi
  case ":$PATH:" in *":$BIN_DIR:"*) ;; *) warn "$BIN_DIR is not on PATH. Add: export PATH=\"$BIN_DIR:\$PATH\"" ;; esac
}

main "$@"
