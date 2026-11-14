#!/usr/bin/env bash
# Verify the omalook tree: one line per gate, ok:/FAIL:, details only for failures.
# Usage: scripts/verify.sh [--no-build]     Exit 0 only if every gate passed.
# Meant to be run by a cheap (Haiku) subagent: it reads only this run's results, so
# its short output is the whole report. Does not deploy or restart anything.
set -u
cd "$(dirname "$0")/.."
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

# Same build-dir rule as build.sh (OMALOOK_BUILD_DIR from env or ~/.config/omalook/build.env).
BUILD_ENV="${XDG_CONFIG_HOME:-$HOME/.config}/omalook/build.env"
if [ -z "${OMALOOK_BUILD_DIR:-}" ] && [ -f "$BUILD_ENV" ]; then
    OMALOOK_BUILD_DIR=$(sed -n 's/^[[:space:]]*OMALOOK_BUILD_DIR=//p' "$BUILD_ENV" | tail -n1 \
        | sed -e 's/^["'"'"']//' -e 's/["'"'"']$//' -e "s|^~|$HOME|")
fi
export CARGO_TARGET_DIR="${OMALOOK_BUILD_DIR:-$PWD/target}"

LOG=$(mktemp -d "${TMPDIR:-/tmp}/omalook-verify.XXXXXX")
trap 'rm -rf "$LOG"' EXIT
FAILED=0

gate() {  # gate <name> <command...>
    local name=$1; shift
    if "$@" >"$LOG/$name.log" 2>&1; then
        echo "ok:   $name"
    else
        echo "FAIL: $name (exit $?)"; tail -n 15 "$LOG/$name.log" | sed 's/^/      /'
        FAILED=1
    fi
}

if [ "${1:-}" != "--no-build" ]; then
    gate build cargo build --release
    # Warnings are information, not a failure; report the count so a jump is noticed.
    w=$(grep -c '^warning' "$LOG/build.log" 2>/dev/null || true)
    echo "info: build warnings: ${w:-0}"
fi
# test_copy_to_clipboard needs a clipboard and hangs headless: skipped on purpose.
# A failing test is re-run once: a pass on retry is reported as WARN (flaky), two failures as FAIL.
# (google_auth listener_returns_the_code... has failed once under load with BrokenPipe, 6/6 alone.)
if cargo test --lib -- --skip test_copy_to_clipboard >"$LOG/lib-tests.log" 2>&1; then
    echo "ok:   lib-tests"
else
    flaky=$(grep -E '^    [a-z_:0-9]+$' "$LOG/lib-tests.log" | tr -d ' ' | tr '\n' ' ')
    if cargo test --lib -- --skip test_copy_to_clipboard >"$LOG/lib-tests2.log" 2>&1; then
        echo "WARN: lib-tests flaky, passed on retry: ${flaky:-unknown}"
    else
        echo "FAIL: lib-tests (failed twice)"; tail -n 15 "$LOG/lib-tests2.log" | sed 's/^/      /'; FAILED=1
    fi
fi
grep -hE '^test result' "$LOG/lib-tests2.log" "$LOG/lib-tests.log" 2>/dev/null | head -1 | sed 's/^/info: /'

if command -v node >/dev/null 2>&1; then
    for t in tests/*.test.js; do gate "node-$(basename "$t" .test.js)" node "$t"; done
else
    echo "WARN: node not found, JS tests skipped"
fi

# Supply-chain gates. Known advisories live in scripts/audit-ignore.txt (and deny.toml); a NEW one fails.
# Offline-tolerant: if the advisory DB cannot be fetched, retry with the cached DB (WARN); no DB at all = WARN.
AUDIT_IGNORES=()
while read -r id _; do
    case $id in RUSTSEC-*) AUDIT_IGNORES+=(--ignore "$id");; esac
done < scripts/audit-ignore.txt

net_gate() {  # net_gate <name> <no-fetch-flag> <cmd...>: run; on fetch failure retry with the flag
    local name=$1 nofetch=$2; shift 2
    if "$@" >"$LOG/$name.log" 2>&1; then echo "ok:   $name"; return; fi
    if grep -qiE "fetch|network|resolve|connect|timed out|unable to update|git operation|failed to load advisory|couldn't" "$LOG/$name.log"; then
        local cmd=("$@"); if [ "$name" = deny ]; then cmd=(cargo deny "$nofetch" "${@:3}"); else cmd+=("$nofetch"); fi
        if "${cmd[@]}" >"$LOG/$name.log" 2>&1; then
            echo "WARN: $name used cached advisory DB"; return
        elif grep -qiE "no such file|not found|no advisory|failed to load advisory|couldn't" "$LOG/$name.log" \
             && ! grep -qE "^(error\[|Crate:|ID:)|vulnerabilit" "$LOG/$name.log"; then
            echo "WARN: $name skipped, no advisory DB available"; return
        fi
    fi
    echo "FAIL: $name"; grep -E "^ *(ID|Crate|Version|Title):|^error" "$LOG/$name.log" | head -n 12 | sed 's/^/      /'; FAILED=1
}

if command -v cargo-audit >/dev/null 2>&1; then
    net_gate audit --no-fetch cargo audit "${AUDIT_IGNORES[@]}"
else
    echo "WARN: cargo-audit not installed, audit skipped"
fi
if command -v cargo-deny >/dev/null 2>&1; then
    # cargo deny takes --offline before `check`; net_gate special-cases that.
    net_gate deny --offline cargo deny check advisories bans licenses sources
else
    echo "WARN: cargo-deny not installed, deny skipped"
fi

echo "info: uncommitted files: $(git status --short | wc -l)"
[ $FAILED -eq 0 ] && echo "RESULT: all gates passed" || echo "RESULT: FAILED"
exit $FAILED
