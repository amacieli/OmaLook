#!/bin/sh
cd $HOME
OMALOOK_PORT=28155 RUST_LOG=omalook=info ${BIN:-/mnt/d/r3-sbx/bin/omalook} > /tmp/backend.log 2>&1 &
PID=$!
python3 /mnt/d/r3-sbx/${PROBE:-probe.py}
echo "--- refused/panic lines:"; grep -E 'refused|panicked' /tmp/backend.log | head -5
kill $PID 2>/dev/null; wait $PID 2>/dev/null
