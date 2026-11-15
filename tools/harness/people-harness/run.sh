#!/bin/sh
cd $HOME
( sleep 4; python3 - <<'P'
import sqlite3,os,time
p=os.environ["XDG_CONFIG_HOME"]+"/omalook/messages.db"
for _ in range(50):
    if os.path.exists(p): break
    time.sleep(0.2)
time.sleep(1)
d=sqlite3.connect(p,timeout=20); d.executescript(open("/mnt/d/r3-sbx/seed.sql").read()); d.close(); print("seeded")
P
) &
OMALOOK_PORT=28177 QT_QPA_PLATFORM=offscreen QML_DIR=/mnt/d/people-harness/qml QML_IMPORT_PATH=/opt/omalook/plugin/omalook-compose/build/qml RUST_LOG=omalook=info,warn timeout 60 $BIN > /mnt/d/people-harness/out.log 2>&1
grep -E "DRV|seeded|ReferenceError|TypeError|Error:" /mnt/d/people-harness/out.log | head -60
