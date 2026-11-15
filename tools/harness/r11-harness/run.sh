#!/bin/bash
# runs INSIDE bwrap (own net namespace: 127.0.0.1:27182 here is the trap, not the live app)
cd /mnt/d/r11-harness
python3 fake.py /mnt/d/r11-harness/hits.jsonl > /mnt/d/r11-harness/fake.out 2>&1 &
FP=$!
sleep 1
OMALOOK_PORT=27999 QML_DIR=/mnt/d/r11-harness/qml QML_IMPORT_PATH=/opt/omalook/plugin/omalook-compose/build/qml QT_QPA_PLATFORM=offscreen timeout 60 quickshell -p /mnt/d/r11-harness/qml > /mnt/d/r11-harness/qs.log 2>&1
echo "quickshell exit $?" >> /mnt/d/r11-harness/qs.log
kill $FP
