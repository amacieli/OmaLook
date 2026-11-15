# Sandbox / UI test harnesses

Scripts copied from the old scratch dirs under `/mnt/d` so they survive a machine move.
Not part of the build. Binaries, captured output, screenshots and tool shims were left out.

| Dir | Purpose |
|---|---|
| `r3-sbx/` | Sandbox differential: `sbx.sh` + `probe.py` + `compare.py` run `reqs-*.json` (9 groups, ~601 requests) against two backends seeded from `seed.sql`. `sbx_ui.sh`/`run_ui.sh` drive the UI. |
| `people-harness/` | Whole-app UI harness: QtTest keys/clicks (`qml/shell.qml`) against the sandboxed backend. |
| `paste-harness/`, `r4photo-harness/`, `r11-harness/`, `r11-base/h/` | Single-feature QML drivers (paste, contact photo, R11) plus fake-server scripts. |
| `r12-sbx/`, `r15-sbx/` | Older probe scripts. |

Caveats: many scripts hardcode `/mnt/d/<dir>` and `/home/adam`; edit paths on a new machine.
The sandbox scripts expect copies of `quickshell`, `wl-copy`, `xclip`, `xdg-open` in a `bin/` dir
and old/new omalook binaries (`omalook-old`, `omalook-new`), which are not committed.
`r4photo-harness` also used fake images (`fake/alice.jpg`, `photo-set.jpg`) that are not committed.
