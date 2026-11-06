# PLAN — Tags and tagging rules: one set of tags, managed in OmaLook, across Exchange, Gmail and the calendar

Status: PLAN ONLY. No code written. Written 2026-11-04. Tracker: OL-040 (umbrella), OL-025 (rules; becomes phases 2-4 here).
Scope: (1) manage tags themselves (list, colour, rename, merge, delete, counts); (2) manage the automatic rules that apply tags
(today: Outlook rules on the Exchange server, edited in Outlook); (3) make the same tags and rules work on every account;
(4) tags on calendar entries.
Vocabulary: **tag** is OmaLook's word. Exchange calls it a *category*, Gmail a *label*; the picker already says so per account
(`docs/PLAN-mail-fields.md`, key `t`, `:tag`).

## 1. What exists today (read from code and the live cache, 2026-11-04)

- Tagging a message works: `t` picker, push queue (`cat_pending`), Exchange `categories` + master list with colours, Gmail user labels
  (`PLAN-mail-fields.md`). **Not built there:** rename / recolour / delete, a tags page, search by tag.
- Rules: nothing. OL-025 is a research stub.
- Calendar: **no category field anywhere** (`calendar_events` table, `calendar_write.rs`, `google_calendar.rs` never mention it).
- Scopes: Graph already has `MailboxSettings.ReadWrite` in the top tier (`src/auth.rs`), which is exactly the permission `messageRules` needs.
  Google has `gmail.modify` but **not** `gmail.settings.basic`, which the filters API needs.

### Real data (read-only query of `~/.config/omalook/messages.db`)
| Account | Messages | Tagged |
|---|---|---|
| exchange-primary | 60,705 | 7,899 (13%) |
| gmail-45ed5f | 873 | 0 |
| gmail-2a8d9f | 5 | 0 |

Top Exchange tags: Family 3,750 · SaintLucia 1,179 · Saint Lucia 1,069 · CARMEL 618 · Civil Air Patrol 368 · Ukraine 347 · IGOR 305 · SASPRO 169
· St Lucia 143 · Receipts 140 · Panama 110 … 30 distinct names.

What the data says about the rules in use (inferred from sender patterns; the real rule list is a phase-0 read):
1. **Mostly sender-based**, sometimes domain-based: `*@carmelbaptist.org` + `notifications@onrealmmail.org` → CARMEL; `*@ncwgcap.org`, `*@capnhq.gov` → Civil Air Patrol;
   `*@3n1hvac.com`, `*@housecallpro.com` → HVAC; `balboabanktrust.com` + `corporacionbct.com` → Panama; `venmo@venmo.com` → BBQ2022.
   So one tag has **several senders/domains**, and a sender can need **two tags** (Family + SaintLucia).
2. **Many senders per tag, long tails** (SaintLucia: gmail.com, jamiefinch.net, viceroyhotelsandresorts.com, floissaclawyers.com …). A rule editor that makes
   you type 40 addresses is not acceptable; it needs "add this sender to rule X" from a message (like `t`, but for rules).
3. **Tags are also applied by hand** (Family from `adam@macielinski.net`, `erika@…`: these are mostly self/family mail; likely manual or a to/from rule).
   Rules and manual tags will coexist; rules must never remove a tag a person added.
4. **Near-duplicate names**: `SaintLucia` / `Saint Lucia` / `St Lucia`, `CARMEL`, `CivilAirPatrol` / `Civil Air Patrol`, `Blue Category`, `$has cal`
   (a system artefact with an Exchange DN as sender, ignore), `Adam Job` / `Job hunting`. A **merge** operation is a real need, not a nicety.
5. Gmail accounts have no tags yet, so for Gmail this is greenfield: tags first, rules second.

## 2. Provider facts that shape the design

### Exchange / Graph (`/me/mailFolders/inbox/messageRules`, permission `MailboxSettings.ReadWrite`; docs: learn.microsoft.com messageRule, "Create rule")
- Rules are **server-side**: they run on delivery for every client, even with OmaLook closed. This is the behaviour the user has today and wants to keep.
- Rules are created on the **Inbox** and run on mail arriving there; they are **not retroactive** (Outlook's "Run rules now" has no Graph equivalent).
- Conditions (`messageRulePredicates`) include `fromAddresses`, `senderContains`, `subjectContains`, `bodyContains`, `bodyOrSubjectContains`, `headerContains`,
  `recipientContains`, `sentToAddresses`, `sentToMe`, `hasAttachments`, `importance`, `categories`, plus exceptions. Actions: `assignCategories`, `moveToFolder`, `markAsRead`,
  `markImportance`, `delete`, `forwardTo`, `stopProcessingRules`.
- Fields `sequence` (order), `isEnabled`, `hasError`, `isReadOnly`. Rules created in Outlook desktop with **client-only** actions are not visible or are read-only: expect some of the
  user's rules to be read-only in OmaLook. Phase 0 finds out how many.
- A category name used by a rule that is not in the master list still works as a string, but shows without a colour. `displayName` of a master category **cannot be changed**
  through Graph (only colour), so **rename = create new + rewrite every message + rewrite rules + delete old**. Verify in phase 0.
- Calendar events carry `categories` too, from the **same master list** (so one tag set serves mail and calendar on Exchange). There are **no rules for events** on any provider.

### Gmail (`users.settings.filters`, scope `gmail.settings.basic`)
- Filters are **server-side**, run on delivery, add or remove labels. Criteria: `from`, `to`, `subject`, `query` (full Gmail search), `negatedQuery`, `hasAttachment`, `size`.
- **Not retroactive** via API; **create and delete only, no edit** (an "edit" is delete + create); limit of 1000 filters.
- `gmail.settings.basic` is **restricted**, the same class as `gmail.modify` that OmaLook already requests, so no new review tier, but every existing account must **sign in again** to grant it,
  and the unverified-app warning (OL-038) appears again. Plan a graceful "tags work, server rules need a sign-in" state, the way the Graph tier fallback already works.
- Labels double as folders in Gmail's own UI. A Gmail tag is a user label (already how `t` works).

### Google Calendar
- **No categories or tags.** Only `colorId` (11 fixed colours) and `extendedProperties.private` (key/value, invisible in Google's UI, syncs to every OmaLook install on the account).
  See decisions Q5.

## 3. Design

### 3.1 Three layers, same shape as distribution lists (local object as source of truth, accounts as mirrors)
1. **Tag** (account-independent): name, colour, optional note. A per-account **mapping** (`tag_map`: tag ↔ Exchange category name / Gmail label id), by exact name, case-insensitive,
   with manual aliases (`St Lucia` → `Saint Lucia`). Unmapped provider tags are listed as "only on this account", one click to adopt or create on the other accounts.
2. **Rule** (account-independent definition, per-account **engine**): conditions + actions + `applies_to` (accounts, default: all) + order + enabled.
   Engine per account is chosen automatically and shown on the rule:
   - **Server** when the provider can express it (Exchange `messageRule`, Gmail `filter`). Runs with OmaLook closed.
   - **Local** when it cannot (conditions Gmail filters lack, anything for calendar, retroactive apply). The daemon evaluates on ingest and on demand.
   The UI shows per rule per account: `server ✓` / `local` / `server (read-only, made in Outlook)` / `needs sign-in`.
3. **Apply** (retroactive): run any rule over existing mail, **dry run first** (match count and a sample of subjects), then queue through the existing `cat_pending` push path.
   Neither provider offers this, so it is a local feature on all accounts.

### 3.2 Rule model (v1)
- Conditions (AND within a rule; OR via several values in one field): from address / domain / contains; to or cc contains; subject contains; body contains (server on Exchange,
  Gmail `query`); has attachment; importance; header contains (e.g. `List-Id`, Gmail `list:`). Exceptions: same fields, for "except when".
- Actions v1: **add tag(s)**, mark read, move to folder/label-archive, stop processing. Delete and forward are **phase 7, off by default, behind a confirm** (see §5 safety).
- A rule never removes a tag it did not add (manual tags survive). Re-running a rule is idempotent.
- Do not double-apply: when a server rule is live on an account, the local engine skips it on ingest (it still does retroactive apply). Adopted Exchange rules are tracked by their server id.

### 3.3 UI
- **Settings → Tags**: one table, rows = tags, columns = colour, per-account presence (✓/–/alias), count of messages, count of rules using it. Keys: rename, recolour, merge into…, delete,
  adopt. Bulk jobs show progress in the bottom bar like "filling in message details".
- **Settings → Rules** (and `:rules`): ordered list with engine badges; editor in the same vim-style form pattern as lists; `Ctrl+Enter` saves; test button = dry run on the cache.
- **From a message**: `T` (capital) = "make a rule from this message": prefilled sender/domain → tag chosen with the `t` picker; shows how many cached messages it would match.
  This is what the real rule list (long sender tails) needs most.
- **Search**: `tag:Family` operator (the "not built" gap in PLAN-mail-fields), `tag:none`.
- **Calendar**: tag chips on events and a tag picker in the event editor; `t` in the calendar view, same picker component.

## 4. Phases (each ends with a build, tests, and a live check by the user; none touches the primary mailbox with move/delete actions)

**Phase 0 — spikes, read-only first (no UI)**
1. `GET messageRules` on the live Exchange account: count, `isReadOnly`, `hasError`, which conditions/actions are in use, `sequence`. Record in the plan (§6 table).
2. Disabled scratch rule `omalook-spike-*` on the Exchange account: create, read, patch, delete round trip; confirm `assignCategories` with a name absent from the master list.
3. Verify `masterCategories` rename behaviour (PATCH displayName rejected?) on a scratch category.
4. Graph event `categories` round trip on a scratch event. Google event `extendedProperties.private` round trip on a scratch event on `gmail-2a8d9f` (the 5-message account).
5. Gmail `settings.filters` create/list/delete on `gmail-2a8d9f` after a re-sign-in with `gmail.settings.basic`; confirm the consent screen and whether the unverified-app warning returns.
6. Measure: how many of the user's rule-assigned tags the cache attributes to a sender rule (precision/recall of "sender → tag" inferred from the 7,899 tagged messages). Tells us how much a **suggest rules from history** feature would find.

**Phase 1 — Tags page** (no rules): list across accounts, counts, recolour, delete, adopt-to-other-accounts, `tag:` search. (Rename and merge dropped by decision Q3.) **Built 2026-11-06**, see OL-040 in the tracker for what was checked; delete = strip from messages via `cat_pending`, then delete on the provider once nothing is pending.
Needs a backup step (copy `messages.db`, as done for mail fields) before the first bulk rewrite.

**Phase 2 — Rules, read-only**: Exchange `messageRules` listed in Settings → Rules, grouped by tag, with a plain-language summary; read-only rules marked. No writes yet. **Built 2026-11-06 (Exchange only):** Gmail filters need `gmail.settings.basic`, which is not requested until the phase-5 scope step-up; engine badges arrive with phase 4. See OL-040.

**Phase 3 — Rule editor, Exchange server engine**: create/edit/reorder/enable/delete Exchange rules (tag, mark read, move, stop). Fetch-before-write because Graph rules have no ETag; detect a rule edited elsewhere (compare to last seen copy) and ask.
`T` from a message. Unsupported conditions disabled with the reason.

**Phase 4 — Local engine + retroactive apply**: SQLite rule tables, evaluation on ingest in the sync daemons, dry-run and apply, per-rule "applied N" stats. This is what makes Gmail and any non-server rule work.

**Phase 5 — Gmail server engine**: scope step-up (`gmail.settings.basic`), filters create/delete (edit = replace), label creation with allowed colours, mapping criteria to Gmail `query`.

**Phase 6 — Calendar tags**: Exchange via `categories` (shared master list); Google per decision Q5; tag chips, picker, `tag:` filter in calendar, local rules for events (Graph and Google both have none).

**Phase 7 — Extras**: import/export (JSON; Outlook rules cannot be exported by Graph so JSON is OmaLook's own), "suggest rules from history" (phase-0 item 6), delete/forward actions behind confirm, MCP: read tools for tags/rules first, write tools gated by the existing policy classes (`src/mcp/policy.rs`).

## 5. Safety rules (earned the hard way: the OL-035 test that deleted 10 DMARC reports)
- Spikes and tests use scratch names `omalook-spike-*`, created **disabled**, and are deleted at the end of the test; list the server's rules before and after to prove nothing else changed.
- No rule with move/delete/forward against the primary mailbox in tests. Tag-only rules there, on a scratch tag.
- Retroactive apply always shows the dry-run count first and refuses when the count is over a threshold (setting, default 1,000) without an explicit confirm.
- Server rule writes: fetch, diff, write, re-fetch, compare. Any mismatch is reported, never silently retried.
- Bulk tag rewrites (merge/rename) take the DB backup first and are resumable and undoable (keep the old-tag list per message until the job completes).

## 6. Phase-0 results (Exchange half run 2026-11-04 with `tools/spike/tags_spike.py`, account adam@macielinski.net; Google half done 2026-11-06)
| Check | Result |
|---|---|
| Exchange rules: count, read-only, conditions/actions in use | **35 rules** (31 enabled, 4 disabled), **2 read-only** (`CP1`, `CP2`: empty conditions and actions, sequence 1-2, probably client-side/system placeholders, must be preserved untouched), 0 with errors. Conditions: `senderContains` 18, `fromAddresses` 13, `subjectContains` 3, `bodyContains` 2, `bodyOrSubjectContains` 1; no exceptions anywhere. Actions: `assignCategories` 28, `stopProcessingRules` 30, `forwardTo` 2, `markImportance` 2, `delete` 1. |
| Q7 answered | **Yes, not only tags**: 2 forward rules (TradeSmith and Income Intelligence mail forwarded to Erika), 1 delete rule (an Instagram sender, body match), 2 importance rules (LifeLock, a test rule). So phase 2/3 must **display and preserve** forward/delete/importance rules from day one; editing them stays behind the confirm gate of phase 7. |
| Shape of the tag rules | 28 tag rules, **one rule per sender or domain**, each with one condition value (most rules carry a single value; max 3 values). 10 rules feed SaintLucia, 3 CARMEL, 3 Civil Air Patrol, 2 each Family/HVAC/Ukraine. 30 of 35 rules stop processing, so **order matters** (first matching tag rule wins and stops the others). Several tags have rules that are disabled (IGOR, 4 rules). **Consequence for the UI: group rules by tag** (one tag, its list of senders) instead of showing 28 separate rules; a rule editor that adds a sender to a tag is the main action. Rule names are Outlook's auto-names ("For all messages from X"). |
| `assignCategories` with a name not in the master list | Rule **accepted and stored** (name returned as written). What a delivered message then shows was not tested. Today every category used by a rule is in the master list. |
| Master category rename | **`PATCH displayName` returns 200 but silently changes nothing** (re-read shows the old name). Colour PATCH works. So rename = create + rewrite + delete, as planned, and a "200" must never be trusted without a re-read. (Rename is out of scope per the user, kept for the record.) |
| Server behaviours seen on rule writes | `sequence` is **assigned by the server** (asked 9999, got 36: appended at the end); condition text is **upper-cased/normalised** on store (`OMALOOK-SPIKE-NOBODY@…`), so compare case-insensitively. **No etag or changeKey on rules**, confirming fetch-before-write plus a stored copy for change detection. Create, get, patch (replace a condition list), delete all work; scratch rule and category deleted, rule and category lists identical before and after (35 and 28). |
| Graph event `categories` round trip | **Works**: create with two names (one unlisted), patch, get, delete all fine; `$filter=categories/any(c:c eq 'Family')` accepted (200) on events. Scratch event removed. |
| Sender→tag coverage in the cache | Enabled sender-only tag rules match 2,839 cached Exchange messages; **2,105 (74%) carry the rule's tag**. By tag: HVAC, Ukraine, AcquisitionAce 100%; Civil Air Patrol 97%; SaintLucia 74%; Family 69%; CARMEL 61%. The gap (734) is mail that predates a rule or arrived elsewhere than the Inbox, so **retroactive apply (phase 4) would tag several hundred existing messages**. Rules explain only 2,105 of the 7,899 tagged messages; the rest was tagged by hand or by rules since deleted, so history-based suggestions (phase 7) have real input but cannot recover a rule list. |
| Google event `extendedProperties.private` round trip (run 2026-11-06, `tools/spike/google_spike.py`, scratch account teamcisconc300@gmail.com) | **Works**: create with `extendedProperties.private.omalook_tags`, read back identical, PATCH changes it, `events.list?privateExtendedProperty=omalook_tags=Family` finds the event (server-side filter works), delete 204. Scratch event removed. |
| Gmail filters round trip + consent wording (same run) | Scope `gmail.settings.basic` **granted** (with `gmail.labels`, `calendar.events`). Filters list on an empty account returns **204 with no body** (handle as empty). Create label + filter (`criteria.from`, `action.addLabelIds`) 200, get returns the same, **edit is impossible: PUT and PATCH both 404**, so edit = delete + create as planned. Delete filter and label 204; filter list identical before and after (0). **The unverified-app warning does appear** (user, 2026-11-06), the same Advanced → Go to omalook click-through as at first sign-in. Not tested: that a delivered message actually gets the label (filters apply on delivery only). |

## 7. Decisions and open questions
Decided (user, 2026-11-04):
- **Q1 server is authoritative on Exchange.** OmaLook edits the same Outlook rules; Outlook keeps working. OmaLook's own rule store holds only what servers cannot run (local engine, other accounts).
- **Q2 one rule definition across accounts**, `applies_to` picks accounts, default all.
- **Q3 no merge, no rename.** The near-duplicate names are the user's own slips from Outlook's poor UI and will be cleaned up by hand later. **Phase 1 drops merge/rename and the bulk-rewrite job**; it keeps list, recolour, delete (with a count and confirm), adopt-to-other-accounts and `tag:` search. The bulk-rewrite safety items in §5 apply only if a merge is ever added.
  OmaLook's tag picker should still warn when a new name is a near-match of an existing tag (case/space/abbreviation), which prevents new duplicates at the source.
- **Q5 Google Calendar tags live in `extendedProperties.private`** (syncs between OmaLook installs, invisible in Google's UI).
- **Q6 Gmail server rules (restricted scope, re-sign-in) only in phase 5**; the local engine covers Gmail until then.

Still open:
- Q4. Gmail tags: plain labels (`Family`, as `t` does today) or a namespace (`Tags/Family`) to keep Gmail's label list tidy? (Plain is the default until answered.)
- Q7. **Answered by phase 0** (see §6): forward x2, delete x1, importance x2 exist alongside 28 tag rules.
