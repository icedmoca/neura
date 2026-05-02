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

log() { printf '\033[1;32m[kcode install]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[kcode install]\033[0m %s\n' "$*" >&2; }
fail() { printf '\033[1;31m[kcode install]\033[0m %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

install_deps_hint() {
  cat >&2 <<'EOF'
Missing dependencies. Install at least: git, curl, cargo/rustc.

Ubuntu/Debian:
  sudo apt-get update && sudo apt-get install -y git curl build-essential pkg-config libssl-dev
  curl https://sh.rustup.rs -sSf | sh

Fedora:
  sudo dnf install -y git curl gcc gcc-c++ openssl-devel pkg-config rust cargo

Arch:
  sudo pacman -S --needed git curl base-devel openssl pkgconf rust
EOF
}

for cmd in git curl; do have "$cmd" || { install_deps_hint; fail "missing command: $cmd"; }; done
if ! have cargo; then install_deps_hint; fail "missing command: cargo"; fi

mkdir -p "$KCODE_HOME" "$MODEL_DIR" "$BIN_DIR"

if [ -d "$SRC_DIR/.git" ]; then
  log "updating source in $SRC_DIR"
  git -C "$SRC_DIR" fetch --depth=1 origin main || git -C "$SRC_DIR" fetch origin
  git -C "$SRC_DIR" checkout main || true
  git -C "$SRC_DIR" pull --ff-only || true
else
  rm -rf "$SRC_DIR"
  mkdir -p "$(dirname "$SRC_DIR")"
  log "cloning $REPO_URL -> $SRC_DIR"
  git clone --depth=1 "$REPO_URL" "$SRC_DIR"
fi

MODEL_PATH="$MODEL_DIR/$MODEL_FILE"
if [ "$SKIP_MODEL" != "1" ]; then
  if [ -s "$MODEL_PATH" ]; then
    log "model already exists: $MODEL_PATH"
  else
    TMP="$MODEL_PATH.tmp.$$"
    URL="https://huggingface.co/$HF_REPO/resolve/main/$MODEL_FILE?download=true"
    log "downloading model from Hugging Face: $HF_REPO/$MODEL_FILE"
    warn "this is a large GGUF file and may take a while"
    curl -L --fail --retry 5 --retry-delay 5 --continue-at - -o "$TMP" "$URL"
    mv -f "$TMP" "$MODEL_PATH"
  fi
  ln -sfn "$MODEL_FILE" "$MODEL_DIR/kcode-oss-20b-mxfp4"
  ln -sfn "$MODEL_FILE" "$MODEL_DIR/gpt-oss-20b-mxfp4_moe.gguf"
  ln -sfn "$MODEL_FILE" "$MODEL_DIR/jcode-gpt-oss-20b.gguf"
fi

log "building Kcode ($BUILD_PROFILE)"
if [ "$BUILD_PROFILE" = "debug" ]; then
  cargo build --manifest-path "$SRC_DIR/Cargo.toml" --bin kcode
  BUILT_BIN="$SRC_DIR/target/debug/kcode"
else
  cargo build --manifest-path "$SRC_DIR/Cargo.toml" --release --bin kcode
  BUILT_BIN="$SRC_DIR/target/release/kcode"
fi

VERSION="kcode-local-$(date +%Y%m%d%H%M%S)"
DEST="$KCODE_HOME/builds/versions/$VERSION"
mkdir -p "$DEST" "$KCODE_HOME/builds/stable"
cp "$BUILT_BIN" "$DEST/kcode"
chmod +x "$DEST/kcode"
cat > "$DEST/jcode" <<'EOF'
#!/usr/bin/env bash
exec "$(dirname "$0")/kcode" "$@"
EOF
chmod +x "$DEST/jcode"
cp "$DEST/kcode" "$KCODE_HOME/builds/stable/kcode.new"
mv -f "$KCODE_HOME/builds/stable/kcode.new" "$KCODE_HOME/builds/stable/kcode"
cp "$DEST/jcode" "$KCODE_HOME/builds/stable/jcode.new"
mv -f "$KCODE_HOME/builds/stable/jcode.new" "$KCODE_HOME/builds/stable/jcode"
ln -sfn "versions/$VERSION" "$KCODE_HOME/builds/current"
printf '%s\n' "$VERSION" > "$KCODE_HOME/builds/current-version"
printf '%s\n' "$VERSION" > "$KCODE_HOME/builds/stable-version"

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

log "installed: $($BIN_DIR/kcode --version 2>/dev/null || true)"
log "binary: $BIN_DIR/kcode"
log "home: $KCODE_HOME"
log "model: $MODEL_PATH"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) warn "$BIN_DIR is not on PATH. Add: export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac
