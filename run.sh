#!/bin/bash
# Run script for OmaLook (Rust backend + QML UI hosted by Quickshell)
# Usage: ./run.sh [--release] [--dev] [--qml-dir <path>]
#
# Features:
# - Hot-reload QML files on save (watches qml/ directory in dev mode)
# - Optional QML directory override for testing
# - Release or debug mode

set -e

# Ensure Rust toolchain is available
if [ -f "$HOME/.cargo/env" ]; then
    source "$HOME/.cargo/env"
fi

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Defaults
BUILD_TYPE="debug"
DEV_MODE=false
PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$PROJECT_DIR"
QML_DIR="$PROJECT_DIR/qml"
BINARY=""

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --release)
            BUILD_TYPE="release"
            shift
            ;;
        --dev)
            DEV_MODE=true
            shift
            ;;
        --qml-dir)
            QML_DIR="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: ./run.sh [--release] [--dev] [--qml-dir <path>]"
            exit 1
            ;;
    esac
done

# Determine binary path. The cargo target dir is configured outside the
# project (.cargo/config.toml), so ask cargo instead of assuming ./target.
TARGET_DIR="$(cargo metadata --format-version 1 --no-deps 2>/dev/null \
    | grep -o '"target_directory":"[^"]*"' | cut -d'"' -f4)"
TARGET_DIR="${TARGET_DIR:-$PROJECT_DIR/target}"
BINARY="$TARGET_DIR/$BUILD_TYPE/omalook"

# Check if binary exists
if [ ! -f "$BINARY" ]; then
    echo -e "${YELLOW}⚠️  Binary not found at $BINARY${NC}"
    echo -e "${YELLOW}Building...${NC}"
    ./build.sh --"$BUILD_TYPE"
fi

echo -e "${GREEN}🚀 OmaLook${NC}"
echo "Binary: $BINARY"
echo "QML Dir: $QML_DIR"
echo "Mode: $([ "$DEV_MODE" = true ] && echo "development (hot-reload)" || echo "production")"
echo ""

# === DEV MODE: File watching for QML hot-reload ===
if [ "$DEV_MODE" = true ]; then
    echo -e "${BLUE}📡 Watching QML directory for changes...${NC}"
    
    # Start background watcher
    if command -v watchexec &> /dev/null; then
        # Use watchexec if available (more efficient)
        watchexec -c -w "$QML_DIR" --debounce 200ms \
            "echo '$(tput setaf 3)⟳ QML files changed, app will reload$(tput sgr0)'" &
        WATCH_PID=$!
        trap "kill $WATCH_PID 2>/dev/null || true" EXIT
    elif command -v inotifywait &> /dev/null; then
        # Fallback: use inotify-tools (less efficient but works)
        (
            while inotifywait -r -e modify "$QML_DIR" 2>/dev/null; do
                echo -e "${YELLOW}⟳ QML files changed, restart app to reload${NC}"
            done
        ) &
        WATCH_PID=$!
        trap "kill $WATCH_PID 2>/dev/null || true" EXIT
        echo -e "${YELLOW}⚠️  Note: Install watchexec for automatic hot-reload on save${NC}"
        echo -e "${YELLOW}   apt install watchexec (or: cargo install watchexec-cli)${NC}"
    else
        echo -e "${YELLOW}⚠️  Watched needed for hot-reload not found${NC}"
        echo -e "${YELLOW}   Install: apt install watchexec inotify-tools${NC}"
        DEV_MODE=false
    fi
fi

# === RUN APPLICATION ===
echo -e "${GREEN}Starting application...${NC}"
echo ""

# Set environment variables for the binary
export QML_DIR="$QML_DIR"
export RUST_LOG="${RUST_LOG:-omarchy_look=debug,info}"

# Run the binary
# The binary starts the backend, then launches the UI with `quickshell -p $QML_DIR`
exec "$BINARY"
