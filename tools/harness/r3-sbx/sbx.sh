#!/bin/sh
# usage: PROBE=probe.py sbx.sh   — runs run.sh inside a no-net, no-dbus sandbox with a throwaway home
UIH=/mnt/d/r3-sbx/home
rm -rf "$UIH"; mkdir -p "$UIH/.config" "$UIH/.cache" "$UIH/run"; chmod 700 "$UIH/run"
exec bwrap --ro-bind / / --dev /dev --proc /proc --tmpfs /tmp --tmpfs /run/user/1000 \
  --tmpfs /home/adam/.config/omalook --tmpfs /home/adam/.local/share/keyrings --tmpfs /home/adam/.cache/omalook \
  --bind "$UIH" "$UIH" --bind /mnt/d/r3-sbx/out /mnt/d/r3-sbx/out --unshare-net --unshare-pid --die-with-parent \
  --unsetenv DBUS_SESSION_BUS_ADDRESS --unsetenv WAYLAND_DISPLAY --unsetenv DISPLAY \
  --setenv HOME "$UIH" --setenv XDG_CONFIG_HOME "$UIH/.config" --setenv XDG_CACHE_HOME "$UIH/.cache" \
  --setenv XDG_DATA_HOME "$UIH/.local/share" --setenv XDG_RUNTIME_DIR "$UIH/run" \
  --setenv QML_DIR /mnt/ai/projects/omalook/qml --setenv PATH "/mnt/d/r3-sbx/bin:$PATH" --setenv PROBE "${PROBE:-probe.py}" --setenv BIN "${BIN:-/mnt/d/r3-sbx/bin/omalook}" --setenv REQS "${REQS:-/mnt/d/r3-sbx/reqs.json}" --setenv OUT "${OUT:-/mnt/d/r3-sbx/out.jsonl}" --setenv SEED "${SEED:-}" \
  sh /mnt/d/r3-sbx/run.sh
