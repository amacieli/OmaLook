# PLAN — Distribution lists: usable from any account, synced upstream where the provider allows

Status: PLAN ONLY. No code written. Written 2026-11-01. Tracker item: OL-012 (user request 2026-10-17).
Scope: named lists of recipients that can be addressed from any From account, edited locally, and mirrored to one or several
signed-in accounts (Gmail first; Microsoft Graph within what its API actually permits). Mail only; the same lists could later serve
calendar attendees and SMS groups, the model leaves room but nothing else is built here.

## 0. The one finding that shapes everything

**Microsoft Graph v1.0 has no supported API for Outlook's personal "Contact Groups" (personal distribution lists).**
- Graph's groups overview marks distribution groups and mail-enabled security groups as read-only; only Microsoft 365 groups and plain security groups are writable.
  <https://learn.microsoft.com/en-us/graph/api/resources/groups-overview?view=graph-rest-1.0>
- A Microsoft Q&A answer states there is no Graph replacement for Outlook personal contact groups and no announced plan; a Stack Overflow report of reading
  `IPM.DistList` items through the beta/migration endpoints returned HTTP 400. (Secondary sources, not a Microsoft statement: re-check in phase 0.)
- What *does* round-trip on Graph is the contact's `categories` field.
  <https://learn.microsoft.com/en-us/graph/api/resources/contact?view=graph-rest-1.0>

**Google can do all of it.** People API `contactGroups` has create/update/delete/list/get, and `contactGroups.members.modify` adds and removes members by
contact resource name; scope `https://www.googleapis.com/auth/contacts`, which OmaLook already requests.
<https://developers.google.com/people/api/rest/v1/contactGroups>, <https://developers.google.com/people/api/rest/v1/contactGroups.members/modify>

So "synced with Graph" cannot mean "creates a real Outlook distribution list". The design below is honest about three different kinds of upstream object and treats the
**local list as the source of truth**, with accounts as optional mirrors.

## 1. What exists today (read from the code 2026-11-01)

- Contacts: `contacts` and `contact_folders` tables (`src/db.rs` ~449-491), `account_id` per row. Graph contacts: **read-only** (`src/providers/contacts.rs`, trait has no write methods).
  Google contacts: **read-only** (`src/providers/google_contacts.rs`). The only write is the local favourite star (`is_favorite`, survives sync).
- `contacts_daemon.rs` syncs every 2 minutes: first run pages everything, later runs fetch only the 50 most recent per folder, and **deletions upstream are never applied**.
- Google labels (contactGroups, user groups only) are read, but **folded into exclusive folders**: a contact appears under its first label only, and label folders fetch nothing. Multi-membership is lost.
- Autocomplete: `AppState.loadComposeContacts()` flattens `GET /contacts?view=all` into `{name,email}` once and effectively never refreshes it; `RecipientField.qml` does a substring match, max 6 suggestions, and forms chips in `addRecipients`/`commit`.
- Compose send: `POST /compose/send` validates and queues the whole payload in the outbox (`compose.rs`, `outbox.rs`); a queued send never changes when data changes later. There is **no recipient cap check**.
- `PeopleView.qml` is read-only: All, Favorites and "Contact lists" (which are really contact *folders*). The word "list" therefore already means something else in the UI.
- Scopes: Graph asks for `Contacts.ReadWrite` in the full tier, but accounts that consented earlier sit on a `Contacts.Read` base tier and would get a 403 on writes (sign-in again). Google already has full `contacts`. No `Group.*` scope on Graph.
- Reusable patterns: the outbox / undo-window worker; `calendar_outbox` (`op` column, one-per-account worker, idempotent); `add_column` migrations; `scoped()` ids for per-account provider ids (`accounts.rs`).
- **No conflict handling anywhere**: no ETag/If-Match/changeKey in the repo; upserts are last-writer-wins.
- MCP exposes contacts read-only (`src/mcp/read.rs`), with policy classes (`policy.rs`).

### Defects found on the way (must be fixed or verified before lists build on contacts)
1. **Suspected, from reading the code, not run:** `contact_folders.id` is a global primary key and Graph's default folder uses the unscoped id `"contacts"`; the upsert's `ON CONFLICT(id)` does not update `account_id`. A **second Graph account's default folder would never sync**. Write a test that reproduces it first.
2. Upstream contact deletions never reach the cache (a deleted contact stays in lists and suggestions forever).
3. Google multi-label membership is discarded (above); lists need it.
4. The compose suggestion array is stale for the whole session.
These become tracker items when confirmed (phase 0). They are prerequisites, not scope creep: a list of people pointing at ghosts is worse than no list.

## 2. Terminology (decide first)

"List" is taken: PeopleView's "Contact lists" are folders. Proposal: rename that section to **Folders**, and use **Lists** for distribution lists
everywhere (UI, commands, docs). Alternative name "Groups" collides with Google "labels" and Microsoft 365 groups; "Lists" matches the TRACKER wording.

## 3. Direction: what the rest of OmaLook and the terminal world suggest

- **Local, keyboard-first, scriptable.** mutt's aliases are a plain file you own, queried by `query_command`, expandable with a keystroke; aerc delegates to `address-book-cmd`
  (`khard`, etc.). <https://neomutt.org/guide/advancedusage.html>, <https://man.archlinux.org/man/aerc-config.5.en>.
  Take: lists are *owned by the user's machine*, not by whichever provider happens to support them; offer **import/export** in the formats those tools use (mutt alias file, vCard `KIND:group`, CSV).
- **Provider capability honesty** (calendar plan, principle 4): per account, show what the account can do and why a link is unavailable.
- **Daemon owns state, UI asks; undo and an outbox for anything that leaves the machine**; SQLite for data, not `settings.toml` (settings stays for preferences).
- **One account-independent object, many optional mirrors** is the same shape as the calendar plan's "calendar provider vs meeting provider" split.
- Compact UI: one chip for a list in the recipient row, not 80 chips.

## 4. Design

### 4.1 Concepts

| Term | Meaning |
|---|---|
| **List** | Account-independent, owned by OmaLook. Name, optional description, members, per-list sending options. Usable from any From account. |
| **Member** | An address (+ display name), optionally tied to a contact (`contact_id`, scoped per account), or another list (nesting). |
| **Link** | Mirror of a list on one account. A list can have zero, one or several links (e.g. Gmail A, Gmail B, Graph). |
| **Remote kind** | What the link maps to upstream: `google_label` (real, two-way), `graph_category` (tag on contacts, two-way, see 4.4), `graph_group_address` (a mail-enabled directory group used as one address, read-only, 4.5), later `carddav_group`. |
| **Unlinked remote group** | A Google label or directory group OmaLook has found but the user has not adopted yet. Listed dimmed; `Enter` adopts it as a new list. Never auto-adopted (labels like "Starred", "Friends", old junk would flood the list view). |

### 4.2 Data model (new tables; SQLite, `CREATE TABLE IF NOT EXISTS` + `add_column`, in `src/db/lists.rs`)

```
dl_lists   (id, name, name_key UNIQUE casefold, description, placement 'as_typed'|'bcc',
            default_from NULL, created_at, updated_at, deleted_at NULL)          -- tombstone for sync
dl_members (list_id, member_key, kind 'address'|'list', address, display_name,
            contact_id NULL, added_at, PRIMARY KEY(list_id, member_key))        -- member_key = lower(address) or list id
dl_links   (id, list_id, account_id, remote_kind, remote_id, remote_name, direction 'two_way'|'push'|'pull',
            state 'ok'|'pending'|'error'|'needs_reauth'|'unlinked_remote', remote_etag, sync_token,
            last_sync_at, last_error)
dl_link_base (link_id, member_key)                                                -- last agreed membership (merge base)
dl_outbox  (id, link_id, op 'create'|'rename'|'add'|'remove'|'delete', payload, state, attempts, send_at, last_error)
contact_group_members (account_id, contact_id, group_id)                          -- Google multi-label membership (phase 0/4)
contacts.categories_json (new column)                                            -- Graph categories (phase 5)
```
Members are stored as **address strings first**, with `contact_id` an optional hint, because (a) a list must work for people who are in no address book,
(b) contact ids are per account and would collide (`scoped()` ids per `accounts.rs`), (c) a contact edited later should not silently rewrite a list.
Display names are refreshed from the contact when the same address exists, never the other way round.

### 4.3 Expansion (the heart of using a list)

Resolution is a **pure function** `resolve(list_ids) -> {to/cc/bcc addresses, warnings}`, in Rust, unit-tested:
- nested lists flattened with cycle detection (a→b→a stops, warns, does not loop);
- case-insensitive address dedupe across the whole message (the same person typed directly and via two lists receives one copy);
- invalid addresses and an empty result produce explicit warnings that block Send (never silently drop a member);
- the sender's own address is removed unless the list has `include_self` (default off for lists, off globally for mutt's `metoo` equivalent).

Where it runs: **at send time in the daemon**, not when the user picks the chip. Consequences:
- A list chip stays one compact chip in compose, so a membership edit before Send is honoured.
- The outbox still stores the final expanded payload, so (as today) edits after Send cannot change a queued message, and Undo restores the **chip**, not 80 loose addresses.
- Result feedback: send toast says `Sent to 14 people (Team 12 + 2)`.
- `compose.rs::validate()` gains a recipient-count check against a provider table. Exchange Online and Gmail both cap recipients per message (about 500; Google also has daily limits). Exact values to be confirmed in phase 0; over the cap is a clear error naming the number, not a provider 4xx after the undo window.
- A **confirm for large audiences** (setting, default 20): `Send to 87 people?` once per compose.

Privacy option per list (`placement`):
- `as_typed` (default): members go where the chip sits (To/Cc/Bcc), visible to each other, like typing the addresses.
- `bcc`: members are moved to Bcc and **To holds the sender's own address**, the standard mailing-list pattern. A real Exchange DL hides its members from recipients; a client-side expansion cannot, so Bcc is the only way to give the same effect. The chip shows a small `bcc` mark.

### 4.4 Sync model (local is the source of truth, accounts are mirrors)

Each link is synced by a **three-way set merge** with the stored base (`dl_link_base`):
```
local_added   = local  − base         remote_added   = remote − base
local_removed = base   − local        remote_removed = base   − remote
new_base = (base ∪ local_added ∪ remote_added) − local_removed − remote_removed
```
Adds and removes are commutative, so the only conflict is "removed on one side, added on the other": **add wins**, logged in a per-link sync log (`:listlog`) so it is never invisible.
Membership is pushed as **deltas**, never as a full overwrite (Google `members.modify` is delta-shaped; Graph category PATCHes are per contact), so a stale local copy cannot wipe the upstream list.
Rename: last writer wins, and a name conflict (Google label names are unique per account) appends ` (2)` and warns.
There is no ETag support in the repo today; the merge design does not need one, but the Google label `etag` and Graph contact `@odata.etag`/`If-Match` should be used on writes for safety (phase 4/5).
Pushes go through a `dl_outbox` modelled on `calendar_outbox` (one worker per account, claim before the network call, retry only transient errors, idempotent per id), debounced 10 s and coalesced, so a burst of edits is one request. Local edits are instant and reversible with Ctrl-Z; the upstream push is the delayed part.
Pulls: Google uses `syncToken` on `contactGroups.list`/`connections.list`; Graph categories ride on the existing contacts sync once it handles deletions (phase 0).

**Per remote kind:**
- **Google label (`google_label`), two-way.** List ↔ contact group. Members must be *contacts in that account's People* (the API adds by contact resource name). A member address with no matching contact has two outcomes, chosen per link:
  `contacts_create = ask` (default: confirm "Create 3 contacts in Gmail so they can join the label?") or `skip` (member stays local-only and is marked `○ not mirrored`). Group membership size limits and `maxMembers` behaviour to be confirmed in phase 0.
  Needs `contacts` write (already granted). System labels (`myContacts`, `starred`) are not offered.
- **Graph category (`graph_category`), two-way, honest about what it is.** A list maps to a contact **category** named `OL: <list>` (Outlook colour categories travel to Outlook web/desktop/mobile). Members = Graph contacts carrying that category; adding a member PATCHes the contact's `categories`, creating the contact first if confirmed. It is not a real Outlook distribution list, and Outlook cannot send to a category. Its value is: durable, visible in Outlook's contact views, and shared between OmaLook installs. Needs full-tier `Contacts.ReadWrite`; accounts on the `Contacts.Read` base tier show `needs_reauth` with the existing sign-in-again path.
  Offered but not default, because users may reasonably prefer a local-only list on Graph; the decision is per link.
- **Directory group address (`graph_group_address`), read-only, pull.** Microsoft 365 groups and mail-enabled distribution groups appear as *addressable entries*: one address that Exchange expands server-side. Members can be listed (`/groups/{id}/members`) for display only. Needs `GroupMember.Read.All` or `Group.Read.All`, which in most tenants need **admin consent**: an optional extra tier, off unless the user opts in, with a clear "your tenant may refuse this" message. Write is not offered (distribution groups are read-only in Graph).
- **CardDAV groups (later track).** vCard `KIND:group` with `MEMBER:` lines (RFC 6350; exact encodings for Apple's `X-ADDRESSBOOKSERVER-MEMBER` variant to be verified) would give iCloud/Fastmail/Nextcloud the same link model; `remote_kind` is already open for it.

**Multiple accounts:** a list may be linked to Gmail A and Gmail B and a Graph account at once; each link merges independently against the one local list, so a change made in Gmail A propagates through the local list to the other links on their next sync. Members that cannot be represented on a given account stay marked `○` on that link only.

**Unlinking vs deleting:** deleting a local list defaults to *unlink only* (upstream objects stay). "Also delete upstream" is a separate choice, requires typing the list name, and runs through the outbox with a normal undo window. Upstream deletion of a mirrored object marks the link `unlinked_remote` and asks, rather than deleting the local list.

### 4.5 Using lists in compose (UI)

- **RecipientField**: lists are a second suggestion source, shown first with a list glyph (e.g. `≡ Team · 12`), then contacts. Typing a list name exactly, or `@name`, makes the match unambiguous. Enter/Tab accepts.
- **The chip**: `≡ Team (12)`, one chip, tooltip lists up to 10 members. A focused chip supports: `e` expand in place into individual address chips (the "mutt expand group" moment), `Enter` open the list in the editor popup, `x`/Backspace remove (existing behaviour).
- **Large or odd lists**: empty list → red chip, Send blocked with the reason; members with invalid addresses → amber chip.
- **From account is independent**: choosing the list never changes From. Optional `default_from` on the list pre-selects an account when the list is added to an *empty* From (never overrides a deliberate choice).
- **Paste**: pasting a list's name in a recipient field behaves like typing it.

### 4.6 Managing lists (UI)

A **Lists** view inside People (alongside All, Favorites, Folders), same pane style as the rest:
- Left: lists with member count, link marks (`G` Google, `M` Microsoft, `○` unmirrored members), sync state glyph. Dimmed rows = unlinked remote groups, `Enter` adopts.
- Right: members, with source (contact / typed / nested list / mirrored-from) and per-link state.
- Keys (to be confirmed free in the People scope against `KEYS.md` at build time): `n` new list, `r` rename, `a` add member (opens a contact picker with type-ahead; a typed address is accepted), `d` remove member, `D` delete list (confirm), `l` link/unlink an account (picker showing each account with its capability and the reason when unavailable), `m` compose to this list, `y` copy addresses to the clipboard, `i`/`o` import/export, `Esc` back. All registered in `AppCommands.qml`, in `?` help, rebindable, palette twins `:newlist`, `:addtolist`, `:linklist`, `:listlog`.
- **From mail**: `:addtolist` on the selected message (or a new binding) offers `sender`, `all recipients`, or `everyone but me` and a list picker (`pick:` palette style, like the category picker). This is the mutt "create alias from sender" habit and the quickest way to build a list.
- Settings → Mail (or a Lists page): large-audience threshold, default placement, `include_self`, adopt-labels behaviour, sync interval.

### 4.7 Import / export (small, high value, no provider risk)
- Export: mutt alias file (`alias name Full Name <a@b>, …`), vCard 4 with `KIND:group` + `MEMBER:mailto:`, CSV (name, address). Import: the same three.
  A user who lives in mutt/aerc/khard can point `query_command`/`address-book-cmd` at the exported file; a Graph user can move lists even though Outlook DLs are not reachable by API.
- Importing is a merge into an existing list or a new list, with a dry-run summary (`+12 new, 3 already present, 1 invalid`).

### 4.8 MCP
Read tools first (`lists_list`, `lists_get`, `lists_resolve` returning the expansion preview) under the existing read class; mutating tools (`lists_create`, `lists_add_member`, …) only in the write class, off by default, per the MCP permission model. Recipient *expansion* is the sensitive part, so `resolve` never sends anything.

## 5. Files likely to change

Rust: new `src/db/lists.rs`, `src/lists.rs` (model, resolve, import/export, merge), `src/lists_daemon.rs` (link sync + outbox worker), `src/providers/lists.rs` (trait) with `google_lists.rs` and `graph_lists.rs`; changes to `src/providers/google_contacts.rs` (keep multi-label membership, sync tokens, writes), `src/providers/contacts.rs` (writes, categories, deletion), `src/contacts_daemon.rs` (deletions, full vs recent fetch), `src/db.rs` (folder id scoping fix, new columns), `src/compose.rs` (expansion hook, recipient cap, placement), `src/models.rs`, `src/settings.rs`, `src/main.rs` (routes: `GET/POST /lists`, `/lists/members`, `/lists/link`, `/lists/resolve`, `/lists/import`, `/lists/export`, `/lists/log`), `src/mcp/*`.
QML: `qml/views/PeopleView.qml` (Lists tab, Folders rename), new `qml/views/ListsPane.qml`, `qml/compose/RecipientField.qml` (list suggestions, chip, expand), `qml/state/AppState.qml` (lists model, refreshing contacts), `qml/keys/AppCommands.qml`, `qml/settings/pages/*`.
Docs: `docs/KEYS.md`, `docs/TRACKER.md`, `docs/PLAN-compose.md` (recipient chips note).

## 6. Phases (each ends with a clean `cargo build --release` + tests + check-in; commits on `main`)

### Phase 0 — Foundations and verification (no new feature)
- Reproduce the folder-id collision with a test, fix by scoping the Graph default folder id (`scoped()`), migrate existing rows. Add upstream deletion handling to contact sync. Refresh `composeContacts` after each contacts sync. Record each as its own tracker item.
- Keep Google multi-label membership in `contact_group_members` (read side only).
- Spikes with a scratch Google label and a scratch Graph category on a test contact: `members.modify` round-trip, label name uniqueness, `maxMembers`; category PATCH with `If-Match`; the real recipient-per-message and per-day caps of both providers; re-check whether any supported Graph path to personal contact groups exists today; whether directory-group reading works without admin consent on Adam's tenant.
- Done when: second Graph account's contacts sync in a test, deletes propagate, and the spike answers are written into section 0/4.4.

### Phase 1 — Local lists core (backend only)
- Tables, `resolve()` with nesting/cycle/dedupe/invalid handling, CRUD routes, expansion preview route. Tests: cycles, case-insensitive dedupe against directly typed addresses, `bcc` placement, `include_self`, empty list, 1,000-member list performance, name uniqueness by casefold.
- Done when: `curl /lists/resolve?id=` returns the expected expansion for a nested list with a duplicate and a cycle.

### Phase 2 — Compose integration
- RecipientField suggestions with list glyph, list chip, tooltip, `e` expand, send-time expansion in `compose.rs`, recipient-cap and large-audience confirm, toast text, Undo restores chips.
- Done when: a list of 3 sends to the right 3 people from either a Gmail or a Graph From account, a duplicate address is sent once, and Undo returns the chip.

### Phase 3 — Lists view and building lists quickly
- Lists tab in People, Folders rename, member add/remove/rename/delete with Ctrl-Z, `:addtolist` from a message, copy addresses.
- Done when: a list is created from a message's recipients and edited entirely from the keyboard.

### Phase 4 — Google link (two-way)
- Google list provider (`contactGroups` CRUD, `members.modify` deltas, sync tokens), adopt flow for unlinked labels, `dl_outbox` worker with debounce, three-way merge with base, `○ not mirrored` and contact-creation confirm, link state display, sync log, `needs_reauth` handling.
- Tests: merge matrix (add/add, remove/remove, add-vs-remove, rename conflict, stale base), request builders as pure functions, outbox retry only on transient errors, no full overwrite ever sent.
- Live: two Gmail accounts linked to one list, edit on each side and in Gmail web, all three converge.

### Phase 5 — Graph link (category) and directory groups
- Graph contact writes (create contact, PATCH categories with `If-Match`), `graph_category` link using the same merge engine; base-tier 403 → reauth path.
- Optional directory-group tier: list mail-enabled groups, addressable-entry display, read-only members; guarded by the admin-consent message.
- Done when: a list linked to a Graph account shows as a category in Outlook web and a change in Outlook web arrives in OmaLook on the next sync.

### Phase 6 — Import / export, MCP, polish
- mutt alias / vCard / CSV import and export with dry-run; MCP read tools then write tools; per-list `default_from`; sync log view; settings page; docs in `KEYS.md`.
- Later track, not scheduled: CardDAV link kind.

## 7. Testing and verification
- `cargo test --lib -- --skip test_copy_to_clipboard`: resolve, merge matrix, import/export round trips, request builders, outbox state machine, migration of existing databases (a copy of the real cache DB, not just fresh ones).
- `node` tests: chip/suggestion ordering, `e` expansion.
- Offscreen QML smoke test of RecipientField with a list chip and an empty list; keyboard-only walkthrough.
- Live matrix on scratch data only: {Gmail, Graph category} × {create, rename, add, remove, delete-and-unlink, upstream delete} and a send from each From account through a list. No test list ever touches a real address book entry that is not tagged `[omalook test]`.

## 8. Risks
- Misreading "synced with Graph": users may expect a real Outlook DL. The UI states plainly what a Graph link is (category tag) and what it is not.
- Contact-driven mirroring forces creating contacts upstream; the confirm step and `skip` option exist so that does not happen silently.
- Sending to a large expanded audience is irreversible after the undo window: the cap check and confirm sit before the outbox.
- Member privacy: `as_typed` shows all addresses to all recipients. Default is `as_typed` for predictability; the `bcc` option exists and is marked on the chip.
- Contact-sync defects (phase 0) may be larger than they look; if so, the phase grows before any list code.
- Admin-consent for directory groups is outside OmaLook's control; the feature must degrade to "not available".
- No ETag layer exists; the delta-only push design is what keeps stale clients from overwriting upstream.

## 9. Open questions for Adam
1. **Naming:** "Lists", with PeopleView's current "Contact lists" renamed to "Folders"?
2. **Graph:** is a category mirror worth building (phase 5), or should Graph accounts simply use local lists plus export/import, with directory groups as read-only addresses?
3. **Privacy default:** `as_typed` (proposed) or `bcc` for new lists?
4. **Large audience confirm:** threshold 20 (proposed)?
5. **Adopting Google labels:** manual adopt only (proposed), or an opt-in "adopt all"?
6. **Contacts for non-contacts:** when a member has no contact on a linked Gmail account: ask (proposed), always create, or always skip?
7. **Priority versus the attachments plan:** the contact-sync defects in phase 0 are worth fixing regardless; shall they be done first and independently?

##10. Answers to section 9.
1. Fine 
2. i do not understand, so leave it alone for now 
3. privacy is defined by whichever field i put the distribution list into.  for example, my civil air patrol emails will use one DL for the bcc line (young cadets) and another DL for the cc line (leadership), with me being the only address on the to: line.
4. add as a setting in the mail settings, and fine to default to 20 - but that would only trigger a warning and confirmation, and not be a full blocker.
5. 
6. make it as setting and default to ask.
7. contact sync defects done as part of this plan only - all the contacts work gets done together.

## 11. Status 2026-11-03: what was built, and the sync proposal

**Decision (Adam, 2026-11-03):** lists are managed in **Settings → Lists**, not in a People tab (this replaces §4.6's location; the People "Contact lists" rename is no longer needed). Privacy follows the field the list sits in (§10.3), so there is no per-list `placement` option.

**Built (phases 1-3, in the working tree):**
- `src/db/lists.rs` (`dl_lists`, `dl_members`; member keyed by lower-case address; names unique ignoring case), routes `GET /lists`, `POST /lists/create|rename|delete`, `POST /lists/members/add|remove`.
- `src/addresses.rs` + `GET /addresses`: every address in From/To/Cc/Bcc of the cached mail; sent mail counts 3, received 1; own addresses, no-reply senders and Exchange legacy DNs excluded; unquoted `Last, First <a@b>` handled. 40,000 newest messages, cached 60 s, about 0.45 s cold on the 61k-message cache.
- `Address.list` in the send payload; `OutgoingMessage::expand_lists` runs in `POST /compose/send` before validation: members replace the entry, one copy per address with To > Cc > Bcc priority, unknown or empty list is an error that names the list. The outbox stores the expanded addresses; the in-memory sent message keeps the chips, so Undo restores them.
- `RecipientField`: lists first, `@` filters to lists, one chip `≡ Name (n)`, red when the list is empty or gone. Settings → Mail: `large_audience_confirm` (default 20, 0 = off) asks "Send to about N people?" once per compose.
- Verified: unit tests (parser, harvest ranking, list CRUD, expansion, settings round trip); a second instance on port 27999 with a copy of the real cache (routes, harvest on 61k messages, send-path errors without sending anything); an offscreen render of the page and the field.

**Left, in order of value:**
1. **Google link (two-way, plan §4.4 phase 4):** a list ↔ a People `contactGroups` label. Local list stays the source of truth; each link keeps the last agreed membership and pushes only deltas (`members.modify`), so a stale copy can never wipe the label. Needs contact resource names for members: ask before creating contacts (setting, default ask, §10.6).
2. **Microsoft Graph (phase 5, still undecided, §10.2):** Graph cannot write Outlook contact groups. Options, cheapest first: (a) none, Graph accounts use local lists; (b) category mirror (`OL: <list>` on contacts); (c) read-only directory groups as addressable entries (admin consent).
3. **Import / export** (mutt alias, vCard `KIND:group`, CSV) works for every provider and needs no API: the practical "sync" for Outlook users.
4. Contact-sync defects from phase 0, done together with the contacts work (§10.7).

