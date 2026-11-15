#!/bin/bash
# INSIDE bwrap: $1 = binary, $2 = label
BIN=$1; L=$2
export HOME=/mnt/d/r12-sbx/home-$L; rm -rf $HOME; mkdir -p $HOME/.config $HOME/.cache $HOME/.local/share
export XDG_CONFIG_HOME=$HOME/.config XDG_CACHE_HOME=$HOME/.cache XDG_DATA_HOME=$HOME/.local/share
export PATH=/mnt/d/r12-sbx/bin:$PATH QML_DIR=/mnt/ai/projects/omalook/qml RUST_LOG=omalook=info,warn
$BIN > /mnt/d/r12-sbx/out/$L.backend.log 2>&1 &
BP=$!
python3 /mnt/d/r12-sbx/${PROBE:-probe}.py /mnt/d/r12-sbx/out/${PROBE:-probe}-$L.json $L $BIN
kill $BP 2>/dev/null; sleep 0.5; kill -9 $BP 2>/dev/null
wait $BP 2>/dev/null
