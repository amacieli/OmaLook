#!/bin/bash
# Build script for OmaLook (Rust + QML)
# Usage: ./build.sh [--release] [--live] [--clean] [--dev-setup]
#
#   --live   build a release binary and install it as the live app in /opt/omalook
#            (override with OMALOOK_LIVE_DIR). Implies --release. The first run
#            asks for sudo once, to create /opt/omalook owned by you; after that no
#            sudo is needed. Does not touch the menu entry. Stops every running OmaLook UI and
#            backend (live and dev copies) before the new files go in, and afterwards starts
#            again whatever was running: the window (with its backend) or, if only the backend
#            was up (Settings → General → Keep running in background), just the backend.
#            `omalook mcp` helpers belong to the AI apps that started them: left alone.
#
# Where cargo builds: ./target by default. To build elsewhere (a fast local disk when the
# clone is on a network share), set OMALOOK_BUILD_DIR, either in the environment or as a line
#   OMALOOK_BUILD_DIR=/some/fast/disk/omalook-build
# in ${XDG_CONFIG_HOME:-~/.config}/omalook/build.env (kept per machine, never committed).

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

# Per-machine build location (see the header). An environment value wins over the file.
BUILD_ENV_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/omalook/build.env"
if [ -z "${OMALOOK_BUILD_DIR:-}" ] && [ -f "$BUILD_ENV_FILE" ]; then
    OMALOOK_BUILD_DIR=$(sed -n 's/^[[:space:]]*OMALOOK_BUILD_DIR=//p' "$BUILD_ENV_FILE" | tail -n1 | sed -e 's/^["'"'"']//' -e 's/["'"'"']$//' -e "s|^~|$HOME|")
fi
BUILD_DIR="${OMALOOK_BUILD_DIR:-$PROJECT_DIR/target}"
export CARGO_TARGET_DIR="$BUILD_DIR"

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


# === RUNNING INSTANCES ===
# pids of processes whose command line is exactly `<exe>` or `<exe> <args>` (not `omalook mcp`).
pids_of_exe() {
    local exe="$1" pid args
    for pid in $(pgrep -f "^$exe( |\$)" 2>/dev/null); do
        args=$(ps -o args= -p "$pid" 2>/dev/null) || continue
        [[ "$args" == "$exe mcp"* ]] && continue
        echo "$pid"
    done
}
pids_of_ui() { pgrep -f "^quickshell -p $1( |\$)" 2>/dev/null || true; }

# Ask politely (TERM), wait up to 8 s, then KILL what is left.
stop_pids() {
    [ $# -gt 0 ] || return 0
    kill "$@" 2>/dev/null || true
    for _ in $(seq 1 40); do
        local alive=0 p
        for p in "$@"; do kill -0 "$p" 2>/dev/null && alive=1; done
        [ $alive = 0 ] && return 0
        sleep 0.2
    done
    kill -9 "$@" 2>/dev/null || true
}

echo -e "${GREEN}🔨 OmaLook Build System${NC}"
echo "Build type: $BUILD_TYPE"
echo "Project dir: $PROJECT_DIR"
echo "Build dir:   $BUILD_DIR"

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
    BINARY="$BUILD_DIR/release/omalook"
    cargo build --release 2>&1 | tee build.log || { echo -e "${RED}❌ Build failed. See build.log.${NC}"; exit 1; }
else
    BINARY="$BUILD_DIR/debug/omalook"
    cargo build 2>&1 | tee build.log || { echo -e "${RED}❌ Build failed. See build.log.${NC}"; exit 1; }
fi

# Check if build succeeded
if [ ! -f "$BINARY" ]; then
    echo -e "${RED}❌ Build failed. Check build.log for details.${NC}"
    exit 1
fi

echo -e "${GREEN}✓ Build successful${NC}"
echo -e "${GREEN}Binary: $(realpath "$BINARY")${NC}"
echo -e "${GREEN}Size: $(du -h "$BINARY" | cut -f1)${NC}"

# === SUMMARY ===
echo -e "\n${GREEN}=== Build Summary ===${NC}"
echo "Output binary: $BINARY"
echo "To run the dev build (close the live app first):  QML_DIR=\$PWD/qml $BINARY"
echo "To install the live app:  ./build.sh --live   (then /opt/omalook/omalook)"

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

    # Remember what is running, then stop all of it (windows first, then backends; the live and
    # the dev copies share port 27182 and the config dir, so none may be left over).
    UI_WAS_UP=false; BACKEND_WAS_UP=false
    LIVE_UI=$(pids_of_ui "$LIVE_DIR/qml"); LIVE_BACKEND=$(pids_of_exe "$LIVE_DIR/bin/omalook")
    DEV_UI=$(pids_of_ui "$PROJECT_DIR/qml")
    DEV_BACKEND=$(for b in "$BUILD_DIR/release/omalook" "$BUILD_DIR/debug/omalook"; do pids_of_exe "$b"; done)
    [ -n "$LIVE_UI" ] && UI_WAS_UP=true
    [ -n "$LIVE_BACKEND" ] && BACKEND_WAS_UP=true
    if [ -n "$LIVE_UI$LIVE_BACKEND$DEV_UI$DEV_BACKEND" ]; then
        echo -e "${YELLOW}Stopping running OmaLook (window: $UI_WAS_UP, backend: $BACKEND_WAS_UP)...${NC}"
        # shellcheck disable=SC2086
        stop_pids $LIVE_UI $DEV_UI
        # shellcheck disable=SC2086
        stop_pids $LIVE_BACKEND $DEV_BACKEND
        sleep 0.5
    fi

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
    if [ ! -d "$PLUGIN_SRC/build" ]; then
        # Fresh clone: configure the plugin (needs cmake and the Qt 6 declarative dev files).
        cmake -S "$PLUGIN_SRC" -B "$PLUGIN_SRC/build" -DCMAKE_BUILD_TYPE=Release > /dev/null \
            || { echo -e "${RED}❌ Could not configure the compose plugin (is cmake + Qt 6 declarative installed?).${NC}"; exit 1; }
    fi
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

    # Launcher: points the app at the installed UI, and refuses to open a second
    # window: it focuses the one that is open instead. A backend running without a
    # window (Settings → General → Keep running in background) is not a window; the
    # binary detects it on port 27182 and attaches a UI to it.
    cat > "$LIVE_DIR/omalook" <<'LAUNCHER'
#!/bin/bash
# OmaLook launcher (written by build.sh --live)
DIR="$(cd "$(dirname "$(readlink -f "$0")")" && pwd)"

# A backend alone (kept running in the background after the window closed) does not count: the
# binary then attaches a new window to it. Only an open window means "already running".
if pgrep -f "^quickshell -p $DIR/qml" > /dev/null; then
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

    # Put back what was running. The launcher starts the backend and a window together.
    if [ "$UI_WAS_UP" = true ] || [ "$BACKEND_WAS_UP" = true ]; then
        RESTART_LOG="${XDG_RUNTIME_DIR:-/tmp}/omalook-restart.log"
        echo -e "${YELLOW}Restarting OmaLook...${NC}"
        setsid -f "$LIVE_DIR/omalook" > "$RESTART_LOG" 2>&1 < /dev/null
        if [ "$UI_WAS_UP" = false ]; then
            # Only the backend was running (no window): there is no backend-only start, so let
            # the launcher bring up both, then close the window again. The backend stays
            # because "keep running in background" is what kept it alive before.
            for _ in $(seq 1 100); do
                [ -n "$(pids_of_ui "$LIVE_DIR/qml")" ] && [ -n "$(pids_of_exe "$LIVE_DIR/bin/omalook")" ] && break
                sleep 0.2
            done
            sleep 1
            # shellcheck disable=SC2046
            stop_pids $(pids_of_ui "$LIVE_DIR/qml")
            sleep 1
            if [ -n "$(pids_of_exe "$LIVE_DIR/bin/omalook")" ]; then
                echo -e "${GREEN}✓ Backend restarted (no window, as before)${NC}"
            else
                echo -e "${RED}⚠ The backend did not stay up; see $RESTART_LOG (is Settings → General → Keep running in background on?)${NC}"
            fi
        else
            for _ in $(seq 1 100); do [ -n "$(pids_of_ui "$LIVE_DIR/qml")" ] && break; sleep 0.2; done
            [ -n "$(pids_of_ui "$LIVE_DIR/qml")" ] \
                && echo -e "${GREEN}✓ OmaLook restarted${NC}" \
                || echo -e "${RED}⚠ OmaLook did not come back; see $RESTART_LOG${NC}"
        fi
    fi
fi
