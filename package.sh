#!/bin/bash
# Package script for OmaLook
# Usage: ./package.sh [--type <format>] [--version <ver>]
#
# Formats:
#   appimage   - Portable Linux AppImage (all distros)
#   aur        - Arch Linux User Repository (build script)
#   pacman     - Arch/Manjaro pacman binary package
#   deb        - Debian/Ubuntu .deb package
#   rpm        - Fedora/RHEL/openSUSE .rpm package
#
# Note: This is Phase 6 (packaging). For now, these are placeholders.
# Full implementation will come when you're ready to distribute.

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${YELLOW}📦 OmaLook Packaging (Phase 6 - Placeholder)${NC}"
echo ""
echo "This script is a skeleton for distributing OmaLook across platforms."
echo "Full implementations will be added when you're ready to package for distribution."
echo ""

# Parse arguments
PACKAGE_TYPE="${1:-all}"
VERSION="${2:-0.1.0}"

case "$PACKAGE_TYPE" in
    appimage)
        echo -e "${BLUE}🖼️  AppImage Packaging${NC}"
        echo "Placeholder: Build portable AppImage for all Linux distros"
        echo "Steps:"
        echo "  1. Build release binary: ./build.sh --release"
        echo "  2. Create AppImage structure with embedded Qt"
        echo "  3. Output: omalook-$VERSION.AppImage"
        echo ""
        echo "Dependencies: linuxdeploy, linuxdeploy-plugin-qt, appimagetool"
        ;;
    
    aur)
        echo -e "${BLUE}📦 AUR Packaging${NC}"
        echo "Placeholder: Arch Linux User Repository PKGBUILD"
        echo "Location: aur/PKGBUILD"
        echo "Workflow:"
        echo "  1. Create aur/ directory with PKGBUILD, .SRCINFO"
        echo "  2. Push to AUR: git push aur"
        echo "  3. Users can: yay -S omalook"
        ;;
    
    pacman)
        echo -e "${BLUE}⚙️  Pacman Binary Package${NC}"
        echo "Placeholder: Arch/Manjaro .pkg.tar.zst"
        echo "Steps:"
        echo "  1. Generate from PKGBUILD: makepkg -si"
        echo "  2. Output: omalook-$VERSION-1-x86_64.pkg.tar.zst"
        echo "  3. Optional: Host on custom repo"
        ;;
    
    deb)
        echo -e "${BLUE}📄 Debian Package${NC}"
        echo "Placeholder: Debian/Ubuntu .deb"
        echo "Structure:"
        echo "  debian/"
        echo "  ├── control         # Package metadata"
        echo "  ├── changelog       # Version history"
        echo "  ├── rules           # Build rules"
        echo "  └── omalook.install"
        echo ""
        echo "Build: dpkg-buildpackage -us -uc"
        echo "Output: omalook_$VERSION_amd64.deb"
        ;;
    
    rpm)
        echo -e "${BLUE}🔴 RPM Package${NC}"
        echo "Placeholder: Fedora/RHEL/openSUSE .rpm"
        echo "Structure:"
        echo "  omalook.spec"
        echo ""
        echo "Build: rpmbuild -ba omalook.spec"
        echo "Output: omalook-$VERSION-1.fc*.x86_64.rpm"
        ;;
    
    all)
        echo -e "${YELLOW}All package formats (placeholder):${NC}"
        echo "  - appimage (portable, all Linux)"
        echo "  - aur (Arch Linux User Repository)"
        echo "  - pacman (Arch/Manjaro binary)"
        echo "  - deb (Debian/Ubuntu)"
        echo "  - rpm (Fedora/RHEL/openSUSE)"
        echo ""
        echo "To package for a specific format:"
        echo "  ./package.sh appimage"
        echo "  ./package.sh deb"
        echo "  ./package.sh aur"
        echo "  etc."
        ;;
    
    *)
        echo -e "${RED}Unknown package type: $PACKAGE_TYPE${NC}"
        echo "Valid types: appimage, aur, pacman, deb, rpm, all"
        exit 1
        ;;
esac

echo ""
echo -e "${BLUE}ℹ️  Phase 6 will implement full packaging${NC}"
echo "   For now, local development uses: ./build.sh && ./run.sh"
