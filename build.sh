#!/bin/bash
# Build script for OmaLook (Rust + QML)
# Usage: ./build.sh [--release] [--live] [--clean] [--dev-setup]
#
#   --live   build a release binary and install it as the live app in /opt/omalook
#            (override with OMALOOK_LIVE_DIR). Implies --release. The first run
#            asks for sudo once, to create /opt/omalook owned by you; after that no
#            sudo is needed. Does not touch the menu entry or a running instance.

set -e

# Ensure Rust toolchain is available
if [ -f "$HOME/.cargo/env" ]; then
    source "$HOME/.cargo/env"
fi

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Detect if we're in the project root
if [ ! -f "Cargo.toml" ] || [ ! -d "qml" ]; then
    echo -e "${RED}Error: build.sh must be run from the project root${NC}"
    exit 1
fi

PROJECT_DIR=$(pwd)
BUILD_TYPE="debug"
CLEAN_BUILD=false
DEV_SETUP=false
LIVE=false
LIVE_DIR="${OMALOOK_LIVE_DIR:-/opt/omalook}"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --release)
            BUILD_TYPE="release"
            shift
            ;;
        --live)
            LIVE=true
            BUILD_TYPE="release"
            shift
            ;;
        --clean)
            CLEAN_BUILD=true
            shift
            ;;
        --dev-setup)
            DEV_SETUP=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: ./build.sh [--release] [--live] [--clean] [--dev-setup]"
            exit 1
            ;;
    esac
done

echo -e "${GREEN}🔨 OmaLook Build System${NC}"
echo "Build type: $BUILD_TYPE"
echo "Project dir: $PROJECT_DIR"

# === DEV SETUP ===
if [ "$DEV_SETUP" = true ]; then
    echo -e "\n${YELLOW}📦 Running development setup...${NC}"
    
    # Check for required Qt 6 development files
    if ! pkg-config --exists Qt6Core Qt6Gui Qt6Qml; then
        echo -e "${RED}❌ Qt 6 development files not found${NC}"
        echo "Install with: sudo apt install qt6-base-dev qt6-qml-module-qtquick"
        exit 1
    fi
    
    # Check for Rust
    if ! command -v cargo &> /dev/null; then
        echo -e "${RED}❌ Rust/Cargo not found${NC}"
        echo "Install from: https://rustup.rs/"
        exit 1
    fi
    
    echo -e "${GREEN}✓ Qt 6 development files found${NC}"
    echo -e "${GREEN}✓ Rust toolchain found ($(rustc --version))${NC}"
fi

# === CLEAN ===
if [ "$CLEAN_BUILD" = true ]; then
    echo -e "\n${YELLOW}🧹 Cleaning build artifacts...${NC}"
    cargo clean
    echo -e "${GREEN}✓ Clean complete${NC}"
fi

# === BUILD ===
echo -e "\n${YELLOW}🏗️  Building ($BUILD_TYPE)...${NC}"

# cargo already runs one job per CPU; `pipefail` makes a failed build fail here instead of being
# masked by `tee` (the old binary would otherwise still exist and be reported as a success).
set -o pipefail
if [ "$BUILD_TYPE" = "release" ]; then
    BINARY="/mnt/d/omalook-build/release/omalook"
    cargo build --release 2>&1 | tee build.log || { echo -e "${RED}❌ Build failed. See build.log.${NC}"; exit 1; }
else
    BINARY="/mnt/d/omalook-build/debug/omalook"
    cargo build 2>&1 | tee build.log || { echo -e "${RED}❌ Build failed. See build.log.${NC}"; exit 1; }
fi

# Check if build succeeded
if [ ! -f "$BINARY" ]; then
    echo -e "${RED}❌ Build failed. Check build.log for details.${NC}"
    exit 1
fi

# Create a symlink named "omalook" (no hyphen) in the project root
ln -sf "$(realpath "$BINARY")" "$(pwd)/omalook"

echo -e "${GREEN}✓ Build successful${NC}"
echo -e "${GREEN}Binary: $(realpath "$BINARY")${NC}"
echo -e "${GREEN}Size: $(du -h "$BINARY" | cut -f1)${NC}"
echo -e "${GREEN}Symlink: $(realpath ./omalook)${NC}"

# === SUMMARY ===
echo -e "\n${GREEN}=== Build Summary ===${NC}"
echo "Output binary: $BINARY"
echo "Executable (no hyphen): ./omalook"
echo "To run: ./omalook"
echo "To run with release build: QML_DIR=qml ./build.sh --release && ./omalook"

# === WEBKIT HTML RENDERER (optional) ===
# omalook-render draws HTML mail with WebKitGTK in its own process. Without it (or without
# libwebkit2gtk-4.1 / gtk3 headers) the app falls back to Qt's rich-text renderer.
RENDER_SRC="$PROJECT_DIR/tools/render/omalook-render.c"
RENDER_BIN="$PROJECT_DIR/bin/omalook-render"
if pkg-config --exists webkit2gtk-4.1 gtk+-3.0 2>/dev/null && command -v gcc >/dev/null; then
    mkdir -p "$PROJECT_DIR/bin"
    gcc -O2 -Wall -o "$RENDER_BIN" "$RENDER_SRC" $(pkg-config --cflags --libs webkit2gtk-4.1) \
        && echo -e "${GREEN}✓ Built omalook-render (WebKitGTK HTML view)${NC}" \
        || echo -e "${YELLOW}⚠ omalook-render failed to build; HTML mail will use the Qt renderer${NC}"
else
    echo -e "${YELLOW}⚠ webkit2gtk-4.1 / gtk3 / gcc not found: HTML mail will use the Qt renderer${NC}"
fi

# === LIVE INSTALL ===
if [ "$LIVE" = true ]; then
    echo -e "\n${YELLOW}🚀 Installing live to $LIVE_DIR...${NC}"

    # One-time: /opt is root's. Create the folder owned by you so later runs need no sudo.
    if [ ! -d "$LIVE_DIR" ] || [ ! -w "$LIVE_DIR" ]; then
        mkdir -p "$LIVE_DIR" 2>/dev/null || {
            echo "Creating $LIVE_DIR (needs sudo once)..."
            sudo install -d -o "$(id -un)" -g "$(id -gn)" -m 755 "$LIVE_DIR"
        }
    fi
    mkdir -p "$LIVE_DIR/bin" "$LIVE_DIR/qml" "$LIVE_DIR/share"

    # Replace the binary by renaming a fresh copy over it: copying onto a running
    # executable fails ("Text file busy"), a rename does not and the running
    # instance keeps its old inode until it is restarted.
    install -m 755 "$BINARY" "$LIVE_DIR/bin/omalook.new"
    mv -f "$LIVE_DIR/bin/omalook.new" "$LIVE_DIR/bin/omalook"

    # The UI. Commons/Ui are links to the Omarchy shell kit that the app creates
    # itself on first start, so leave them out of the copy and out of --delete.
    rsync -a --delete --exclude Commons --exclude Ui "$PROJECT_DIR/qml/" "$LIVE_DIR/qml/"

    # The compose editor is a native QML plugin (OmaLook.Compose). The app looks for it at
    # <parent of qml>/plugin/omalook-compose/build/qml, so mirror that layout. Without it the
    # reply / reply all / forward / new buttons cannot open the editor.
    PLUGIN_SRC="$PROJECT_DIR/plugin/omalook-compose"
    if [ -d "$PLUGIN_SRC/build" ]; then
        cmake --build "$PLUGIN_SRC/build" -j"$(nproc)" > /dev/null || { echo -e "${RED}❌ Compose plugin build failed.${NC}"; exit 1; }
    fi
    if [ -d "$PLUGIN_SRC/build/qml" ]; then
        mkdir -p "$LIVE_DIR/plugin/omalook-compose/build/qml"
        rsync -a --delete "$PLUGIN_SRC/build/qml/" "$LIVE_DIR/plugin/omalook-compose/build/qml/"
    else
        echo -e "${RED}❌ Compose plugin not built ($PLUGIN_SRC/build/qml missing).${NC}"; exit 1
    fi

    if [ -x "$RENDER_BIN" ]; then
        install -m 755 "$RENDER_BIN" "$LIVE_DIR/bin/omalook-render.new"
        mv -f "$LIVE_DIR/bin/omalook-render.new" "$LIVE_DIR/bin/omalook-render"
    else
        rm -f "$LIVE_DIR/bin/omalook-render"
    fi

    install -m 644 "$PROJECT_DIR/packaging/omalook.svg" "$LIVE_DIR/share/omalook.svg"

    # Launcher: points the app at the installed UI, and refuses to start a second
    # copy (they would fight over port 27182 and the database): it focuses the
    # window of the one that is running instead.
    cat > "$LIVE_DIR/omalook" <<'LAUNCHER'
#!/bin/bash
# OmaLook launcher (written by build.sh --live)
DIR="$(cd "$(dirname "$(readlink -f "$0")")" && pwd)"

if pgrep -f "^$DIR/bin/omalook" > /dev/null; then
    addr=$(hyprctl clients -j 2>/dev/null | jq -r '.[] | select(.class=="org.quickshell" and .title=="OmaLook") | .address' | head -n1)
    [ -n "$addr" ] && hyprctl dispatch focuswindow "address:$addr" > /dev/null 2>&1
    exit 0
fi

export QML_DIR="$DIR/qml"
export RUST_LOG="${RUST_LOG:-omalook=info,warn}"
exec "$DIR/bin/omalook" "$@"
LAUNCHER
    chmod 755 "$LIVE_DIR/omalook"

    echo -e "${GREEN}✓ Live install complete${NC}"
    echo "Launcher: $LIVE_DIR/omalook"
    echo "Binary:   $LIVE_DIR/bin/omalook ($(du -h "$LIVE_DIR/bin/omalook" | cut -f1))"
    if pgrep -f "^$LIVE_DIR/bin/omalook" > /dev/null; then
        echo -e "${YELLOW}OmaLook is running: restart it to pick up this build.${NC}"
    fi
fi
