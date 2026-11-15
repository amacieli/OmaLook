#!/bin/bash
# INSIDE bwrap: real UI (offscreen quickshell) against the real new backend, then a second launch (UI only)
UIH=/mnt/d/r12-sbx/home-ui
rm -rf /mnt/d/r12-sbx/home-ui; mkdir -p $UIH/.config $UIH/.cache $UIH/.local/share
export HOME=$UIH XDG_CONFIG_HOME=$UIH/.config XDG_CACHE_HOME=$UIH/.cache XDG_DATA_HOME=$UIH/.local/share
export QML_DIR=/mnt/ai/projects/omalook/qml RUST_LOG=omalook=debug,warn QT_QPA_PLATFORM=offscreen
export QML_IMPORT_PATH=/opt/omalook/plugin/omalook-compose/build/qml
timeout 45 /mnt/d/r12-sbx/omalook-new > /mnt/d/r12-sbx/out/ui-run.log 2>&1 &
P1=$!
sleep 20
timeout 15 /mnt/d/r12-sbx/omalook-new > /mnt/d/r12-sbx/out/ui-run2.log 2>&1
echo "second exit $?" >> /mnt/d/r12-sbx/out/ui-run2.log
wait $P1
echo "exit $?" >> /mnt/d/r12-sbx/out/ui-run.log
