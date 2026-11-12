# PLAN — Contacts: a full keyboard-driven address book (view, create, edit, organise) with a two-way link to lists

Status: PLAN ONLY. No code written. Written 2026-11-11. Tracker item: OL-043. Related: OL-012 (distribution lists), OL-040 (tags/categories), OL-021 (calendar editor, the model for the editor and outbox), OL-042 (list import/export).
Request (user, 2026-11-11): two-way link for lists; keyboard-friendly, with the keys and habits already built for messages; edit and add contacts; every field Graph allows; a full keyboard way to update and add contacts; a phased plan that covers what Outlook's contacts do. **Gmail is a separate, later phase** (reconcile after Graph works).

Facts below marked **(verify)** come from Microsoft's documentation or my memory of it, not from a live call. Phase 0 settles each one before code depends on it.

## 1. Where we are (read from the code, 2026-11-11)

- **Read only.** `src/providers/contacts.rs` (`ContactsProvider`: folders, folder contacts, full walk) and `google_contacts.rs`; `contacts_daemon.rs` syncs; no create, update or delete anywhere. Graph scope `Contacts.ReadWrite` is already in the full tier (`auth.rs`); accounts on the base tier have `Contacts.Read` only.
- **Lossy model.** `models::Contact` keeps: id, display name, given name, surname, company, job title, `emails: Vec<String>` (no per-address name), phones as `{kind, number}`, addresses **flattened to one text line**, folder id, created/modified. The `SELECT` asks for 16 fields. Everything else Graph returns is dropped, so a write built on this model would erase data.
- **No keyboard cursor in `PeopleView.qml`** (its own comment: "the list has no keyboard cursor"; `j`/`k` scroll 60 px). View (all / favorites / contact lists), sort (first / last / company / recent), account filter and favourites (a local flag, `is_favorite`) exist. Colours are still hard-coded.
- **Known sync defects** (list plan phase 0): the second Graph account's default folder id collides with the first (`DEFAULT_FOLDER_ID = "contacts"`, not scoped by account), upstream deletions are only purged after a *complete* walk, and there is no delta sync (every 6 h full walk).
- **No change-key or ETag handling anywhere** in the repo for contacts (calendar and rules solved this per feature; contacts must too).
- Reusable pieces: the calendar outbox (`calendar_outbox.rs`: undo window, claim before network, retry only transient errors, one worker per account), the editor pattern (`EventEditor.qml`, `draft.js`, `when.js`), the key registry (`Commands.js`, `AppCommands.qml`, scopes, chords, which-key, palette, conflict checker), the tag picker and tag store (OL-040), `RecipientField.qml`, the list tables (`dl_lists`, `dl_members`).

## 2. What Graph v1.0 lets us store (contact resource, read 2026-11-11)

Typed properties: `title`, `givenName`, `middleName`, `surname`, `generation` (suffix), `initials`, `nickName`, `fileAs`, `displayName`, `yomiGivenName`, `yomiSurname`, `yomiCompanyName`; `emailAddresses[]` (each `{name, address}`; Outlook shows three, and the resource also lists `primaryEmailAddress`, `secondaryEmailAddress`, `tertiaryEmailAddress` **(verify what the array and these three do on write)**); `imAddresses[]`; `businessPhones[]`, `homePhones[]`, `mobilePhone` (one); `homeAddress`, `businessAddress`, `otherAddress` (each `street, city, state, postalCode, countryOrRegion`); `companyName`, `department`, `jobTitle`, `officeLocation`, `profession`, `manager`, `assistantName`; `birthday`, `spouseName`, `children[]`, `personalNotes`, `businessHomePage`; `categories[]`; `parentFolderId`; read-only `id`, `changeKey`, `createdDateTime`, `lastModifiedDateTime`; `photo` (separate endpoint `/photo/$value`, GET and PUT).
Mechanisms: `If-Match`/`changeKey` for writes **(verify)**, delta per folder, open extensions, single- and multi-value extended properties (MAPI), contact folders with child folders (create, rename, delete).
Gaps against Outlook's own form, reachable only through MAPI extended properties **(verify each property tag and that it round-trips)**: fax, pager, car, company-main, assistant, ISDN/radio/telex/TTY phone numbers, wedding anniversary, personal web page, gender, the "this is the mailing address" flag. Phase 6, optional.
Traps already known: later updates can overwrite `displayName` with a generated value unless every update sends it; arrays (`emailAddresses`, phones, `categories`, `children`) are replaced whole on PATCH, so a write must start from a fresh read; `id` changes when an item moves between folders unless `Prefer: IdType="ImmutableId"` is sent (also decides whether a move can be a move; **verify that v1.0 has a move for contacts at all**, else move = create in the target + delete the source, with the same id-change handling).

## 3. Design principles

1. **Full fidelity first.** The cache stores the whole Graph contact JSON (`data` column) next to the denormalised columns the list needs for sort and search. A save sends only the fields the user changed, built from the fresh server copy, never from the lossy model.
2. **Writes go through an outbox, with an undo window.** Same discipline as the calendar outbox: queue, wait `[mail] send_delay_secs`, claim, send once, read back, verify, store the server's copy. `Ctrl-Z` takes it back and reopens the editor with what was typed.
3. **Never overwrite a newer server copy.** Each queued write carries the `changeKey` the user saw; if the server's differs, the write stops with a `conflict` the user resolves (show both, keep mine / keep theirs / merge fields), as the Exchange rules editor does.
4. **The keyboard is the interface.** Every action has a key, a palette command and an ex name, in registry scopes, with the conflict checker passing. Mouse works but is never required.
5. **Reuse what messages taught.** `j k gg G Ctrl-d Ctrl-u`, `/` search, `Enter` open, `Tab` panes, `x` delete + `Ctrl-Z`, `t` tags, `Ctrl-Shift-V` move, marks for bulk, `A` account cycle (as in the calendar), `c`/`n` new (as in the calendar), `e` edit, `y` copy, `r` refresh.
6. **Pull in the background, UI instant** (user principle 2026-11-10): the list reads the cache; the daemon walks and delta-syncs.
7. **Local lists stay the source of truth** for list membership (OL-012); Graph is a mirror.
8. **A write is never repeated blindly.** Unclear results are reported as failed and the cache re-reads the server.

## 4. Data model

- `contacts`: add `data TEXT` (full Graph JSON as last read), `change_key TEXT`, `photo_etag TEXT`, `sync_state TEXT` (`ok | pending | failed | conflict`), keep the current columns as a denormalised projection (rebuilt from `data`; emails and phones searchable text added for search). Migration through `add_column` (the race-tolerant helper); a one-time re-walk fills `data` for existing rows (the daemon does it in the background).
- `contact_folders`: add `account_id` to the key (fixes the id collision); `change_key`.
- `contact_outbox` (per account; mirrors `calendar_outbox`): `id, account_id, op (create | update | delete | move | photo | folder_create | folder_rename | folder_delete | category), contact_id, base_change_key, payload JSON, queued_at, due_at, state (pending | sending | sent | failed | conflict | cancelled), error, transaction_id`.
- `contact_sync` (delta links per folder, last full walk) next to the existing `account_flags`.
- Lists (phase 3): `dl_links` (list id, account id, kind `graph_category`, `category_name`, `state`), `dl_link_base` (last agreed membership), `dl_link_log` (per-link sync log), as in `PLAN-distribution-lists.md` §4.4.

## 5. Keyboard model

New registry scopes `people`, `people/list`, `people/detail`, `contact/edit` (the editor owns the keyboard like `EventEditor`). Provisional bindings; phase 1 runs them through `Cmd.validate` and the existing conflict checks before they are final.

**People list** (cursor, like the message list)

| Key | Does |
|---|---|
| `j` `k`, `gg` `G`, `Ctrl-d` `Ctrl-u`, `PgUp` `PgDn` | Move the cursor |
| `Enter`, `l` | Show / hide the detail pane; `h`, `Esc` back |
| `Tab` `Shift-Tab` | Next / previous pane (list, detail) |
| `/` | Search (name, email, phone, company, notes, tag; same prompt and history as mail); `n`-style next match not needed, the list filters |
| `c`, `n` | New contact (account = the filtered one, else the cursor's, else the last used) |
| `e` | Edit the contact under the cursor |
| `x` | Delete (moves to the server's Deleted Items; undo window; `Ctrl-Z`) |
| `f` | Favourite (local flag, stays local) |
| `t` | Categories picker (same widget and colours as message tags) |
| `Ctrl-Shift-V` | Move to a contact folder (picker) |
| `Space`, `v` | Mark / visual range; `x`, `t`, `Ctrl-Shift-V`, `a` act on the marks |
| `a` | Add to list (picker of lists, `:addtolist`); `A` cycles the account filter (as in the calendar) |
| `w` | Write mail to this contact (opens compose with the address; a picker when there are several) |
| `S` | Text this contact (phone picker; KDE Connect SMS view) |
| `m` | Schedule a meeting with this contact (opens the event editor with the address; needs calendar phase "attendees") |
| `y` / `Y` | Copy the primary address / copy the contact as vCard |
| `o` | Open: web page, or the address in a map (picker) |
| `s` | Cycle sort (name, last name, company, recent) |
| `V` | Cycle the view (all, favourites, a list, a folder) |
| `r` | Refresh |
| `Ctrl-Z` | Undo the last queued write (same bar as the calendar: "Contact queued. Ctrl-Z undo") |

**Detail pane**: `j` `k` move between fields; `Enter` on an email composes, on a phone texts, on an address opens the map, on a URL opens it; `y` copies the field; `e` edits starting at that field.

**Editor** (`contact/edit`, over the People view like the event editor)

| Key | Does |
|---|---|
| `Tab` / `Shift-Tab` | Next / previous field |
| `Ctrl-Down` / `Ctrl-Up` (also `PgDn` / `PgUp` on a section header) | Next / previous section |
| `Ctrl-Enter` | Save: queued, undo window, then sent |
| `Esc` | Close; once edited it asks, a second `Esc` discards |
| `Ctrl-K` | Add-field picker with typeahead (`birth`, `fax`, `spouse`, `second email`): shows only fields not yet present |
| `Ctrl-N` | Add another row in the current group (email, phone, IM, child) |
| `Ctrl-D` | Remove the current row (asks nothing: the editor is undoable until saved) |
| `Ctrl-Shift-Up/Down` | Reorder rows (first email = primary) |
| `Left` `Right` `Space` | Change a choice (phone type, address kind, account, folder) |
| `Ctrl-Z` (after save) | Take the queued write back and reopen the editor |

Smart entry, as `when.js` does for dates: the Name field takes a whole name (`Dr. Jane Q. Smith Jr.`) and splits it live into title / first / middle / last / suffix, shown beneath; phone numbers are normalised on leaving the field; an address takes one line (`12 Rue X, Paris 75001, France`) and fills the five parts, shown for correction; birthday takes `3 mar 1980`, `1980-03-03`, `mar 3` (no year); categories take typed names with the near-duplicate warning from the tag picker. Live check line: "a contact with this address already exists: Jane Smith (Enter = open it)" before saving a duplicate.
Layout: Outlook's sections in one scrolling form, only filled fields and the basics shown (Name, Email, Phone, Company/Title, Notes); everything else behind `Ctrl-K`. Sections: Name (title, first, middle, last, suffix, nickname, file-as, phonetic), Email (up to three typed rows with display names), Phone (business, home, mobile; typed rows), IM, Addresses (home, business, other; one-line entry), Work (company, department, job title, office, profession, manager, assistant), Personal (birthday, spouse, children), Web (business home page), Categories, Folder and Account, Photo (path or clipboard), Notes.
Rebinding follows the existing `[keys]` table; `docs/KEYS.md` gets a Contacts section in every phase that adds keys.

## 6. Writing and syncing (Graph)

- Create: `POST /me/contacts` or `/me/contactFolders/{id}/contacts`, with `transactionId` kept in the outbox so a retry after a lost answer is detected by reading the folder (Graph has no idempotency key for contacts **(verify)**; if none, a retry first looks for a contact created after `queued_at` with the same display name and address).
- Update: fresh `GET`, compare `changeKey` with the one the user saw, send a PATCH of the changed fields (always including `displayName`), read back, verify exactly the intended change, store the server copy. Arrays are rebuilt from the fresh read plus the user's edit.
- Delete: `DELETE` (to Deleted Items); a "delete permanently" action only behind a typed confirm, phase 4.
- Folders: `POST /me/contactFolders` (and child folders), `PATCH` rename, `DELETE` (empty or with confirm).
- Photo: `PUT /me/contacts/{id}/photo/$value` (JPEG, size cap checked locally), read lazily and cached on disk.
- Sync: delta per folder (`/contactFolders/{id}/contacts/delta`, with `contacts` for the default folder **(verify the exact path)**), stored delta link, full walk every 6 h as the safety net, deletions applied from the delta feed, a dead delta link falls back to a full walk. Rows with a queued write are skipped by the pull (as mail skips unpushed read changes) until the write finishes.
- Auth tiers: an account on the `Contacts.Read` tier shows the editor as read-only with "sign in again to allow editing" (existing re-consent path, as calendar does).
- Errors are shown in plain words next to the item (`short_error` pattern), failed and conflicting writes are listed on the People view with `Ctrl-Z`/retry/discard.

## 7. The two-way list link (Graph, `graph_category`)

Reminder of the shape forced by Graph (see `PLAN-distribution-lists.md` §0): Graph v1.0 cannot create Outlook's personal "Contact Groups", so the real upstream object for a list is a **contact category**. A list is linked to a category name; the members of the list are the contacts carrying it. Outlook web, desktop and mobile show the category on each contact and can filter by it (Outlook cannot send to a category; OmaLook can, because the list expands at send time).

- **Merge:** the three-way set merge of the list plan §4.4 (base, local, remote; adds and removes commute; the only conflict is remove-vs-add, add wins and the sync log says so). Deltas only, never a full overwrite.
- **Push:** list member added = the contact's `categories` gets the name (PATCH of that contact only, fresh read first, `changeKey` checked); removed = the name taken off. Goes through the contact outbox (debounced 10 s, coalesced per contact).
- **Pull:** from the contact sync (delta). A contact that gains or loses the category in Outlook changes the list on the next pull.
- **Members without a contact:** a list member is an address; to carry a category the address needs a contact. Setting `contacts_create` = `ask` (default: "Create 3 contacts in Exchange so they can join this list?"), `skip` (member stays local, marked `○ not mirrored`), `always`. Creating one is a contact create through the outbox with just the address and the name.
- **Category list pollution (check in phase 0):** categories are normally a mailbox-wide master list shared with mail, and OmaLook's Tags page already manages that list. Phase 0 verifies whether a contact can carry a category that is *not* in the master list (Outlook shows it uncoloured), so list mirrors need not appear in the mail tag picker. If they must be in the master list, the Tags page hides the list-owned names behind a marker and the `t` picker on messages leaves them out.
- **Naming (decision needed, Q1):** `OL: <list>` (safe, obvious, recognisable in Outlook), or the plain list name. A user-visible prefix prevents a clash with a category the user already has (an existing `Family` tag).
- **Link UI (Settings → Lists, and `L` in People):** per list a "Mirror on: [Exchange account ✓]" row, state (`linked`, `needs_reauth`, `n not mirrored`), "Sync log", link and unlink (unlink leaves the category on the contacts; "also remove it from the contacts" is a separate typed-confirm).
- **Adopt:** a category already on contacts can be adopted as a new list (members = the contacts), like the Google "adopt label" flow in the list plan.
- **Safety:** every link write is checked by reading the contact again; a stale local copy can never wipe a category from contacts it did not change; the pull never deletes a local list.
- **Gmail** later: `contactGroups` (list plan §4.4), reusing the same merge engine, in its own phase.

## 8. Phases (each ends with a clean `cargo build --release`, tests, `scripts/verify.sh`, a live check where it touches the provider, a tracker update and a check-in; commits on `main`)

### Phase 0 — Spikes and foundations (no new feature)
- Live spikes on the real Exchange account in a **scratch contact folder** (all items named `omalook-spike-*`, removed at the end, the real folders compared before and after as in OL-040 phase 0): create with every typed field; PATCH semantics for arrays and `displayName`; `emailAddresses` count and the three `*EmailAddress` fields; `If-Match` with `changeKey`; delta (default folder and a child folder); move (exists? id change; ImmutableId header); folder create, rename, delete; photo PUT and GET; categories on a contact that are not in the master list; each MAPI tag in §2 (round trip); `/me/people` and `/users` search without admin consent (feeds phase 6); behaviour on a base-tier account (403 shape).
- Fix the sync defects: scope default-folder ids by account (migration of existing rows), apply upstream deletes from delta, store delta links; `composeContacts` refreshed after each sync.
- Schema migration to `data` + `change_key`; provider `fetch` asks for every field (`$select` removed or complete); the re-walk fills `data`.
- Done when: a second Graph account's contacts sync in a test, a contact deleted in Outlook disappears at the next delta, the spike answers are written into §2 and §6 of this file, and every row of the cache has full JSON.

### Phase 1 — Read side that deserves the keyboard
- A real cursor in `PeopleView` (the message-list keys), `Enter` detail pane showing **every** field present (sections as in the editor), `/` search over all fields, account filter `A`, sort `s`, view `V`, marks (`Space`, `v`), copy `y`/`Y`, `w` (compose to), `S` (text), `o` (map / URL), photo display from cache. Colours and fonts from `Commons.Color`/`Style` (finishes the OL-014 follow-up).
- Registry scopes and ex commands, `KEYS.md` Contacts section, which-key and palette entries, `Cmd.validate` clean.
- Tests: `tests/contacts.test.js` (smart name split, address one-liner, phone normalisation, birthday text), Rust tests for the projection from `data`.
- Done when: the whole People view can be used without the mouse and every field Graph returned is visible.

### Phase 2 — Create, edit, delete (Graph), with undo and conflicts
- `contact_outbox` + worker (create, update, delete), `contacts_write.rs` (draft → Graph body, diff against the base, `verify_*`), routes (`POST /contacts/save|delete|undo`), `ContactEditor.qml` + `contact_draft.js` with every field of §5, Ctrl-K add-field picker, smart name/address/phone/birthday entry, duplicate-address check, folder and account choice, photo by path, undo bar, failed/conflict listing, `changeKey` conflict dialog (keep mine / theirs / field-by-field).
- Tests: request bodies (golden JSON for each field kind), diff and array rebuilding, `verify_*` against shapes Graph really returns (including the `name` it adds to addresses, as with rules), outbox discipline (claim once, transient-only retry, crash leaves `sending` reported), conflict path.
- Live check on the scratch folder first, then one real contact created, edited, and removed by hand (not by script), confirmed in Outlook web.
- Done when: a contact with every field type is created and edited from the keyboard alone, appears in Outlook web identical, and an edit made in Outlook web in between produces a conflict instead of an overwrite.

### Phase 3 — Two-way lists link (Graph category mirror) — the headline request
- Link tables, link and unlink UI in Settings → Lists and `L` in People, the merge engine as a pure function (`list_sync.rs`: base/local/remote, tested with the matrix: add/add, remove/remove, add-vs-remove, stale base, rename), push via the contact outbox, pull from the contact sync, sync log, `contacts_create` ask/skip/always, adopt-a-category, `○ not mirrored`, reauth state.
- Tests: the merge matrix, request builders as pure functions, a stale local copy never removes a category it did not change, outbox retry only for transient errors.
- Live: link a scratch list to a scratch category; edit on each side (OmaLook; Outlook web) and watch both converge; remove from Outlook, add in OmaLook concurrently (add wins, logged).
- Done when: one list stays in step with an Outlook category in both directions and the log explains each change.

### Phase 4 — Organise: folders, move, categories, bulk, permanent delete
- Contact folders: create, rename, delete, nested; move (or copy+delete if Graph has no move) with ImmutableId handling; `t` categories on contacts through the shared tag store and colours; bulk actions on marks (delete, tag, move, add to list); permanent delete behind a typed confirm; a "Recently deleted" view (Deleted Items contact folder) with restore.
- Done when: a list of 50 contacts can be tagged, moved and added to a list in one keyboard pass, each step undoable until sent.

### Phase 5 — Outlook-parity actions
- Photos: set from file, clipboard or URL; shown in list and detail; remove.
- Quick create: from a message (`:addcontact` on the sender, prefilling name and address), from an invite attendee, from a calendar event; "add to existing contact" when the address is new on a known name.
- Duplicate finder and merge (same address, same name, same phone): a review list, field-by-field merge preview, merge = one update + one delete, undoable until sent (Outlook's "Link contacts").
- Birthdays (and anniversaries when phase 6 gives them): optional annual all-day calendar entries through the existing calendar writer, off by default.
- vCard and CSV: export the marked contacts or a folder as vCard 3/4; import vCard and Outlook CSV with a dry-run summary and duplicate handling (shared with list import/export, OL-042).
- Schedule a meeting with a contact (`m`) and mail a list or a marked set (`w`).
- Print/view modes comparable to Outlook's card, business card and phone-list views: three list layouts (phase 5 only if wanted; the field chooser of `PLAN-mail-list-views.md` is the model).

### Phase 6 — Directory, autocomplete and the rest of Outlook's fields
- Address book beyond contacts: organisation directory search (`/users`, `/me/people`) as a read-only source behind a setting, if phase 0 shows it works without admin consent on the user's tenant; compose autocomplete merges contacts, harvested addresses, lists and directory entries, with the source shown.
- MAPI-backed fields from §2 (fax, pager, car, company main, assistant phone, anniversary, personal web page, gender, mailing-address flag), each only if its phase-0 round trip passed, all shown in the editor under `Ctrl-K`.
- Custom fields: a `[contacts] custom_fields` setting backed by an open extension on the contact, shown in editor and detail, searchable.
- Done when: a field entered in Outlook desktop (fax, anniversary) is visible and editable in OmaLook and survives a round trip.

### Phase 7 — MCP and wrap-up
- MCP read tools (`contacts_search`, `contacts_get`, `lists_list`) then gated write tools (`contacts_create`, `contacts_update`, list membership) under `src/mcp/policy.rs` classes, never `delete` without the confirm class.
- Settings → People page (defaults: new-contact account and folder, name order, duplicate check on/off, `contacts_create` mode), docs, tracker closed.

### Later track (not scheduled): Gmail / Google contacts
Reconcile the same editor and list link with Google People (`contactGroups`, `members.modify`, `connections` sync tokens, the Google-specific fields, contact photos, "other contacts"), using the merge engine of phase 3 and the plan in `PLAN-distribution-lists.md` §4.4. CardDAV after that.

## 9. Testing and verification
- Every phase: build, `scripts/verify.sh`, `qmllint`, node tests for the pure JS, offscreen render of the new views, a scratch instance (port 27999, copy of the real cache; **remember it syncs the real accounts, treat it as live for reads and never point write tests at real folders**).
- Writes: only to a scratch contact folder, named `omalook-spike-*`, with a before/after comparison of the real folders (identical), and a manual one-contact check on the real folder at the end of phase 2.
- A write is not "done" until the contact was read back from the server and compared.

## 10. Open questions (answers wanted before the phase that needs them)
1. **List mirror naming** (phase 3): `OL: <list>` category, or the plain list name? Recommended: `OL: <list>`.
2. **Move semantics** (phase 4): if Graph has no contact move, is "create in target + delete source" acceptable (the Outlook id changes; the favourite flag and list membership follow by address)?
3. **Fields shown by default in the editor** (phase 2): Name, Email, Phone, Company/Title, Notes with the rest behind `Ctrl-K`, or everything always visible? Recommended: the former.
4. **Undo window for contacts**: the same `send_delay_secs` as mail and calendar, or none for contact edits (instant save, conflicts still checked)?
5. **Directory (GAL) read** (phase 6): is a tenant consent request acceptable if phase 0 shows it is required, or skip it?
6. **Favourites**: stay local forever, or mirror as a category (e.g. `OL: ★`) so other OmaLook installs see them? Recommended: local until asked.
7. **Birthdays to the calendar** (phase 5): wanted at all?
