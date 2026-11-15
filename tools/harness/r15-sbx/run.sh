#!/bin/sh
cd $HOME
OMALOOK_PORT=28155 RUST_LOG=omalook=info /mnt/d/r15-sbx/bin/omalook > /tmp/backend.log 2>&1 &
PID=$!
python3 /mnt/d/r15-sbx/${PROBE:-probe.py}
echo "--- backend log lines mentioning secret/.ssh/token:"; grep -c -E 'id_ed25519|\.ssh|report\.txt|[0-9a-f]{64}' /tmp/backend.log || true
echo "--- stub log:"; cat /tmp/stub.log 2>/dev/null
kill $PID 2>/dev/null; wait $PID 2>/dev/null
