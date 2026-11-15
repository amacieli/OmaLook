#!/bin/bash
# INSIDE bwrap: another program holds 127.0.0.1:27182; OmaLook must exit non-zero and launch no UI
SQH=/mnt/d/r12-sbx/home-squat
rm -rf /mnt/d/r12-sbx/home-squat; mkdir -p $SQH/.config
export HOME=$SQH XDG_CONFIG_HOME=$SQH/.config PATH=/mnt/d/r12-sbx/bin:$PATH QML_DIR=/mnt/ai/projects/omalook/qml RUST_LOG=omalook=info,warn
rm -f /mnt/d/r12-sbx/out/stubs.log
python3 -c "
import socket,time
s=socket.socket(); s.bind(('127.0.0.1',27182)); s.listen(5)
s.settimeout(25)
hits=0
try:
  while True:
    c,_=s.accept(); hits+=1; c.settimeout(1)
    try: open('/mnt/d/r12-sbx/out/squat-hits.log','ab').write(c.recv(4096)+b'\n----\n')
    except Exception: pass
    c.close()
except Exception: pass
" &
SP=$!
sleep 0.5
rm -f /mnt/d/r12-sbx/out/squat-hits.log
timeout 20 /mnt/d/r12-sbx/omalook-new > /mnt/d/r12-sbx/out/squat.log 2>&1
echo "omalook exit $?" >> /mnt/d/r12-sbx/out/squat.log
kill $SP
echo "quickshell stub launched: $(test -s /mnt/d/r12-sbx/out/stubs.log && echo yes || echo no)" >> /mnt/d/r12-sbx/out/squat.log
echo "squatter received: $(cat /mnt/d/r12-sbx/out/squat-hits.log 2>/dev/null | head -c 300)" >> /mnt/d/r12-sbx/out/squat.log
