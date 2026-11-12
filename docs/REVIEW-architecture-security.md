# Architecture and Security Review: Prompt and Plan of Action

Purpose: hand this document to a higher-level Claude model (Opus-class, long context, tool access to the
repo) to run a full architectural and security review of OmaLook. Part 1 is the prompt to paste. Part 2 is the
recommended plan of action (phases, order, and what to produce). Part 3 is the output contract and
follow-up handling.

Snapshot when written: ~30.8k lines of Rust in 69 files, a QML UI (71 files), a C++ QML plugin, and a C
WebKitGTK render helper. Treat the numbers as approximate and re-measure.

---

## Part 1: The prompt (paste as-is)

```
You are performing a full architectural review and security review of OmaLook, a native Linux
mail/calendar/contacts/tasks/SMS client. Repo: /mnt/ai/projects/omalook (remote github.com/amacieli/OmaLook).
It is a single-user desktop app, not a multi-tenant service, so judge threats against that model.

READ FIRST, in this order, before forming opinions:
1. AGENTS.md (working notes, hard-won rules; README.md is stale, ignore it)
2. docs/COMPONENT_REFERENCE.md (dependency/version master guide)
3. docs/TRACKER.md (known issues, stable OL-### IDs; do not re-report what is already tracked, reference it)
4. docs/privacy-policy.md (what the app claims it does with data; check the code against the claims)
5. docs/PLAN-*.md and docs/KEYS.md as needed for the subsystem you are examining

SYSTEM SHAPE
- Rust backend daemon (src/): launcher + HTTP backend on 127.0.0.1:27182 (src/main.rs), sync daemons
  (email_daemon, calendar_daemon, contacts_daemon, sms_daemon), providers (src/providers/: Microsoft Graph,
  Gmail, Google Calendar/Contacts/Tasks), SQLite+FTS5 cache (db.rs, db/), outbox/compose/attachments,
  rules engines (rules.rs, local_rules.rs, gmail_filters.rs), SMS via KDE Connect over D-Bus (zbus), and an
  MCP server (src/mcp/: server, rpc, read, admin, policy, token) that exposes mailbox data to AI agents.
- Auth: OAuth2 for Microsoft and Google (auth.rs, google_auth.rs, qt_bridge/auth_bridge.rs); tokens in the
  Linux Secret Service via the keyring crate (token_store.rs, keyring_mgr.rs). Service name is
  "omarchylook" on purpose; do NOT recommend renaming it.
- UI: Qt/QML under Quickshell (qml/), cxx-qt bridge (src/qt_bridge/), C++ plugin (plugin/omalook-compose/).
  HTML mail is rendered by a separate WebKitGTK helper (tools/render/omalook-render.c, binary in bin/) to PNG,
  or sanitised by qml/mail/format.js. Qt Text must never fetch remote URLs; QtWebEngine must never be used
  inside Quickshell (OL-004).
- Packaging: packaging/ (including a bundled Google OAuth client JSON), build.sh, package.sh, /opt/omalook
  live install.

TREAT AS UNTRUSTED INPUT: every byte of mail (headers, HTML, attachments, filenames, calendar invites,
contact fields, SMS bodies, tag/category names), every provider API response, anything arriving on the
loopback HTTP port or the MCP interface, and the MCP clients themselves (an AI agent connected over MCP can
be prompt-injected by mail content).

SECURITY REVIEW: investigate at least these, and add what you find
A. Loopback HTTP server (main.rs): hand-rolled HTTP parsing; responses carry `Access-Control-Allow-Origin: *`.
   Determine what any web page in the user's browser could do (CORS, DNS rebinding, simple cross-origin POSTs,
   missing Host/Origin checks, missing auth token), which endpoints mutate state or return mail data, and
   request size/slowloris/thread-exhaustion limits. Other local users and processes on the same machine count.
B. MCP server (src/mcp/): authentication (token.rs), the read/admin split, policy.rs enforcement, whether a
   prompt-injected agent can exfiltrate mail, send mail, delete mail, change rules, or widen its own
   permissions. Look for confused-deputy and TOCTOU problems, and token storage/permissions/rotation.
C. Credentials and tokens: keyring usage, fallback paths when Secret Service is unavailable, anything written
   to disk/logs/env/process args in plaintext, refresh-token handling, scope minimality versus the privacy
   policy, OAuth flow correctness (PKCE, state, redirect listener binding and lifetime, code interception by
   other local processes), the clipboard helpers (xclip/wl-copy/xsel) in auth_bridge.rs, and the bundled
   Google client secret in packaging/ (assess real impact for an installed-app client; do not paste secret
   values into your report).
D. Untrusted content rendering: HTML sanitiser (qml/mail/format.js) bypasses, the render helper's WebKit
   settings (JS off? network off? file:// access? sandboxing? how it is spawned, input channel, size and
   time limits, crash handling), link handling and xdg-open invocation (scheme allow-list, argument
   injection), remote-content/tracking-pixel leakage, cid:/data: URIs.
E. Attachments and filesystem: attach_spool.rs, attachments.rs, filename sanitisation, path traversal,
   symlink races, temp file modes, where files land, auto-open behavior, size limits, zip/decompression bombs.
F. SQLite and data at rest: SQL injection (any string-built SQL, especially FTS5 MATCH query construction in
   search.rs and rule/tag queries), DB and cache file permissions, whether mail is stored unencrypted
   (state this as a design trade-off with a recommendation, not as a defect), cache retention/purge.
G. Rules, outbox and write paths: can a malicious message trigger rule actions that forward, delete, or leak?
   Outbox replay/duplicate send, header injection in compose (CRLF in To/Subject/filenames), reply-all and
   Bcc handling correctness, invite (invites.rs) handling and join_link.rs URL extraction.
H. SMS/KDE Connect D-Bus: trust of D-Bus peers, message injection, sending paths.
I. Supply chain and build: Cargo.lock review (run cargo audit / cargo deny if available), duplicate HTTP stacks
   (ureq 2.x and reqwest 0.11 both present; old oauth2 4.4, rusqlite 0.30, keyring 2.1), the TLS backend in
   use, pinned versions, build.sh/package.sh (sudo use, curl|sh, writes to /opt), unsafe blocks, and C/C++
   code memory safety (omalook-render.c, documenthandler.cpp).
J. Privacy: compare actual network egress (every outbound host in the code) with docs/privacy-policy.md.
   Report any telemetry, third-party fetches, or remote image loading not stated there.
K. Logging: tokens, message bodies, addresses, or auth codes in logs, eprintln!, build logs committed to the
   repo (build.log, build-output.log).

ARCHITECTURE REVIEW: evaluate and give concrete recommendations on
1. Process and trust boundaries: backend vs Quickshell UI vs render helper vs MCP clients. Are the boundaries
   where the threat model wants them? What runs with what privileges?
2. Concurrency and async: sync (ureq) and async (reqwest/tokio) mixed; tokio features are limited
   (rt, time, macros, sync). Thread-per-connection HTTP; Send/Sync boundaries; lock ordering and shared
   state (SQLite connection handling, WAL, busy timeouts); cancellation and shutdown; daemon supervision.
3. Sync correctness: Graph /delta and Gmail history handling, token-expiry and retry/backoff behavior,
   idempotency of the outbox/calendar_outbox, conflict handling for two-way edits (contacts, calendar, tags),
   data loss on crash mid-sync, purge rules ("purge only trusts a completed first walk").
4. Provider abstraction (src/providers/): does the trait boundary hold, or does provider-specific logic leak
   into db.rs/main.rs/UI? Cost of adding a third provider.
5. Module structure: main.rs (~3000 lines) and db.rs (~3200 lines) are too large; propose a decomposition
   with seams, not a rewrite. Error-handling consistency (anyhow vs thiserror vs unwrap/expect/panic in
   daemon paths; a panic in a sync thread should not silently kill sync).
6. API design: the loopback HTTP surface versus the cxx-qt bridge versus MCP: three ways in. Is the
   duplication justified? Is there a single authorization/validation choke point?
7. Performance and scalability against the stated targets (launch to refreshed mail < 2 s; search p50 < 50 ms
   on 60k rows): obvious hot spots, N+1 queries, unbounded memory on large mailboxes/attachments.
8. Testability and maintainability: test coverage by subsystem, what cannot be tested today and why, the
   known hang in test_copy_to_clipboard (OL-005), docs drift (README is stale), dead code left from the old
   Python backend.
9. Failure modes: offline start, expired/revoked tokens, Secret Service locked, DB corruption, provider
   rate limiting (429/Retry-After), clock skew.

METHOD AND RULES
- Evidence first. Every finding must cite file:line and show the relevant code or the command output that
  proves it. If you did not verify it, label it "hypothesis" and say how to verify. Never invent line numbers.
- Where practical, prove it: write a minimal PoC or failing test in a scratch directory (not in the repo
  tree) and run it. A finding with a working PoC outranks one without.
- Run what exists: `cargo clippy`, `cargo audit`/`cargo deny` if installed, `cargo test --lib -- --skip
  test_copy_to_clipboard` (NEVER run the full suite without that skip; it hangs), `node tests/*.test.js`.
  Build output goes to $OMALOOK_BUILD_DIR, not ./target on this machine, see AGENTS.md.
- Do not modify tracked source, restart the running OmaLook app, run `./build.sh --live`, touch
  ~/.config/omalook or the real keyring, send mail, or call live provider APIs. Test against the dev build
  on an alternate port (OMALOOK_PORT) with throwaway data only. Ask before anything that deviates.
- Do not commit or push. Do not read or print secret values from the keyring, config, or packaging JSON.
- Separate: (1) confirmed vulnerabilities, (2) hardening/defense-in-depth, (3) architectural debt,
  (4) things that are fine and why. Do not pad with generic best-practice lists; if a category has no
  findings, say what you checked and that it was clean.
- Rate each finding: severity (Critical/High/Medium/Low/Info) by realistic exploitability under the
  single-user desktop threat model, plus confidence (Confirmed/Likely/Hypothesis) and effort (S/M/L).
- Do not recommend renaming the keyring service, adding feature branches, or enabling QtWebEngine.

DELIVERABLES
Write to docs/REVIEW-<yyyy-mm-dd>.md (the only file you may create in the repo) with:
1. Executive summary (<= 1 page): top 5 risks, overall posture, and the single best next action.
2. Threat model: assets, actors, trust boundaries, an ASCII data-flow diagram.
3. Findings table, then one section per finding: ID (SEC-### / ARCH-###), title, severity/confidence/effort,
   location, evidence, impact, exploit scenario, recommended fix (with a code sketch where useful), and how to
   verify the fix.
4. Architecture assessment: per-topic verdict for items 1-9 above, with a proposed target structure and an
   incremental migration path.
5. Prioritised remediation roadmap in phases, each item small enough to be one commit, in the order
   described in the plan of action.
6. Appendix: commands run, tool versions, what was not covered and why.
Finish with a short list of questions for the maintainer where intent is ambiguous.
```

---

## Part 2: Suggested plan of action

The reviewer should work in these phases. Each phase ends with notes appended to a working file in a scratch
directory, so that the final report is assembled from verified material rather than memory.

### Phase 0: Orientation (30–45 min of reading, no judgments yet)
- Read the documents listed in the prompt. Build a one-page map: processes, ports, files, secrets, network
  destinations.
- Measure the code: `tokei` or `wc -l` per module; list `unsafe` blocks, `unwrap()`/`expect()`/`panic!` counts
  per file, every `Command::new`, every `TcpListener`/`bind`, every SQL string built with `format!`.
- Run the baseline: `cargo check`, `cargo clippy`, `cargo audit`, the allowed test command. Record what is
  already red before touching anything.
- Enumerate outbound hosts (grep for URLs and base-URL constants) for the privacy comparison.

### Phase 1: Attack surface, highest value first
Examine in this order, because these are the entry points reachable without trusting mail content:
1. **Loopback HTTP server** (A): list every route, its method, what it reads/mutates, and whether any
   authentication exists. Start the dev build on an alternate port and test with curl: bad Host header,
   cross-origin `Origin`, oversized body, slow client, malformed request line. Note the wildcard CORS header.
2. **MCP server** (B): map tools to permissions, read policy.rs end to end, try to get from "read" to "admin"
   or to an exfiltration path. Check token creation, storage mode (0600?), and comparison (constant time).
3. **OAuth and token storage** (C): trace a sign-in from button press to keyring write; check the redirect
   listener, PKCE/state, fallback storage, logs.

### Phase 2: Untrusted content pipeline
4. HTML mail: sanitiser (`qml/mail/format.js`) and render helper (`tools/render/omalook-render.c`).
   Build an adversarial corpus (script tags, `<meta refresh>`, `<link>`/CSS `@import`, `<img src>` remote and
   `file://`, SVG, `cid:` abuse, huge dimensions, malformed UTF-8, deeply nested tables) and run it through the
   render helper directly. Verify no network fetch and no local file read (use `strace -f -e trace=network,openat`
   or a namespace with no network).
5. Attachments, filenames, invites, join links, contact/calendar fields, SMS: follow each from provider
   response to DB to UI to filesystem or `xdg-open`.
6. Compose/outbox: header injection, recipient handling, idempotency.

### Phase 3: Data layer and sync
7. SQL construction, FTS5 query building, file modes, retention.
8. Sync correctness: read delta/history code paths and the failure-injection points (kill mid-sync, 401, 410
   Gone on a delta token, 429). Write unit tests with a fake provider where the trait allows it.
9. Concurrency: lock graph, shared connection use, thread lifetimes, panic containment.

### Phase 4: Architecture synthesis
10. With the facts from phases 1–3, judge the boundaries and the module structure. Propose the target layout
    and a migration order that keeps the app shippable at every commit (for example: extract an HTTP router
    module with an auth/validation layer first, then split `db.rs` by domain, then slim `main.rs`).
11. Cross-check the architecture against the stated performance targets and the failure modes list.

### Phase 5: Report and roadmap
12. Assemble `docs/REVIEW-<date>.md` per the deliverables list.
13. Roadmap ordering rule: (1) anything Confirmed and High or above; (2) cheap hardening with large risk
    reduction (for example an auth token plus Host/Origin check on the loopback server, removing wildcard
    CORS); (3) structural changes that unlock the other fixes; (4) debt that only costs time.

### Suggested model and cost handling
- Run the review on the strongest model available (Opus-class) for phases 1, 2, 4 and 5; judgment-heavy work.
- Phase 0 mechanical sweeps (greps, counts, inventories) can be delegated to a cheaper model, per AGENTS.md
  delegation rules, provided it returns raw evidence (file:line), not conclusions.
- Do not delegate the auth/token, async/`Send`, and FFI reviews; keep them on the main reviewer.
- Expect a large context load: read modules in the order above rather than the whole tree, and use
  `grep`/`rg` to navigate. `db.rs` and `main.rs` should be read by section.

---

## Part 3: After the review

1. Open one TRACKER.md entry per accepted finding (new stable OL-### IDs; never reuse), linking to the
   relevant REVIEW section.
2. Fix Confirmed High/Critical items first, one commit each, stating what was verified. Re-run the PoC to show
   it no longer works.
3. Add regression tests for each security fix (HTTP Origin/Host rejection, sanitiser corpus, filename
   sanitisation, SQL/FTS5 quoting).
4. Update `docs/privacy-policy.md` if the egress audit finds a mismatch, and refresh the stale `README.md`.
5. Re-run this review (Part 1 prompt, scoped to changed areas) after the remediation phases land.

## Known items the reviewer should start from (hypotheses, unverified when this doc was written)
- Loopback HTTP responses send `Access-Control-Allow-Origin: *` (src/main.rs, many sites). Whether any
  endpoint is sensitive and reachable cross-origin is exactly what Phase 1 must settle.
- A Google OAuth client JSON is bundled in `packaging/`. For installed apps this secret is not truly secret,
  but confirm scope, redirect URIs, and that nothing user-specific is bundled.
- `build.log` and `build-output.log` sit in the repo root; check they hold no secrets or personal paths and
  whether they are tracked.
- Two HTTP client stacks (ureq 2.x and reqwest 0.11) and several old major versions (oauth2 4.4, keyring 2.1,
  rusqlite 0.30, thiserror 1.x).
- `main.rs` and `db.rs` each exceed 3000 lines.
- `notes/` is gitignored; verify nothing sensitive from it is tracked in history.
