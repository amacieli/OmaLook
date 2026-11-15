#!/bin/sh
cd $HOME
OMALOOK_PORT=28155 QT_QPA_PLATFORM=offscreen QML_IMPORT_PATH=/opt/omalook/plugin/omalook-compose/build/qml RUST_LOG=omalook=debug,warn timeout 30 ${BIN} > /mnt/d/r3-sbx/out/ui.log 2>&1
echo "requests: $(grep -c 'http <- ' /mnt/d/r3-sbx/out/ui.log)  refused: $(grep -c 'refused' /mnt/d/r3-sbx/out/ui.log)  panics: $(grep -c panicked /mnt/d/r3-sbx/out/ui.log)"
