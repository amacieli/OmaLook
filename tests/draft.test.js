const fs = require('fs');
const src = fs.readFileSync(__dirname + '/../qml/calendar/draft.js', 'utf8').replace(/^\.pragma.*$/m, '');
const D = new Function(src + ';return {stamp,buildRequest,conflicts,canCreate,chooseAccount,cycleAccount,pendingRaw,pendingMark,' +
    'attendeeList,inviteeCount,reachLine,mergePeople,stateOf,changedKeys,isRecurring,editRefusal,removeKind,changePath,' +
    'changeMessage,removeLine,applyChanges,buildPatch}')();
let n = 0;
function eq(a, b, m) { n++; if (JSON.stringify(a) !== JSON.stringify(b)) { console.error('FAIL', m, '\n got', JSON.stringify(a), '\nwant', JSON.stringify(b)); process.exit(1); } }

const when = { ok: true, start: new Date(2026, 9, 28, 10, 0), end: new Date(2026, 9, 28, 10, 30), allDay: false };
eq(D.stamp(when.start), '2026-10-28T10:00:00', 'stamp');

let r = D.buildRequest({ accountId: 'exchange-a', subject: ' Plan ', notes: 'n', when, location: ' ', reminder: 15, showAs: 'busy', repeat: null, transactionId: 'tx' });
eq(r.ok, true, 'ok');
eq(r.body, { account_id: 'exchange-a', subject: 'Plan', body: 'n', start: '2026-10-28T10:00:00', end: '2026-10-28T10:30:00', all_day: false, show_as: 'busy', transaction_id: 'tx', reminder_minutes: 15 }, 'body without location/recurrence');
eq(D.buildRequest({ accountId: 'x', when, reminder: 0 }).body.reminder_minutes, 0, 'reminder 0 is kept (at time of event)');
eq('reminder_minutes' in D.buildRequest({ accountId: 'x', when, reminder: null }).body, false, 'no reminder when null');

const rep = { ok: true, pattern: { type: 'weekly', interval: 1, daysOfWeek: ['wednesday'] }, range: { type: 'endDate', endDate: '2027-10-28' } };
r = D.buildRequest({ accountId: 'x', when, repeat: rep, location: 'Room 4B' });
eq(r.body.recurrence, { pattern: rep.pattern, range: rep.range }, 'recurrence passes through in the shape Rust reads');
eq(r.body.location, 'Room 4B', 'location');
eq('recurrence' in D.buildRequest({ accountId: 'x', when, repeat: { ok: true, pattern: null, range: null } }).body, false, 'repeat none');

eq(D.buildRequest({ accountId: '', when }).ok, false, 'no account');
eq(D.buildRequest({ accountId: 'x', when: { ok: false, error: 'ambiguous' } }), { ok: false, error: 'ambiguous' }, 'bad when');
eq(D.buildRequest({ accountId: 'x', when, repeat: { ok: false, error: 'bad repeat' } }), { ok: false, error: 'bad repeat' }, 'bad repeat');

// conflicts
const ev = (t, h1, m1, h2, m2, allDay) => ({ title: t, allDay: !!allDay, startDate: new Date(2026, 9, 28), startMin: h1 * 60 + m1, endMin: h2 * 60 + m2 });
const evs = [ev('Standup', 9, 30, 10, 15), ev('Later', 10, 30, 11, 0), ev('Holiday', 0, 0, 0, 0, true), ev('Before', 9, 0, 10, 0)];
eq(D.conflicts(evs, when.start, when.end, false), ['Standup'], 'overlap only; touching ends do not count; all-day ignored');
eq(D.conflicts(evs, when.start, when.end, true), [], 'all-day entry has no conflict line');

// accounts
const acc = [{ id: 'gmail-g', provider: 'gmail', email: 'g@x', signed_in: true }, { id: 'exchange-a', provider: 'exchange', email: 'a@x', signed_in: true },
             { id: 'exchange-b', provider: 'exchange', email: 'b@x', signed_in: true }, { id: 'exchange-c', provider: 'exchange', email: 'c@x', signed_in: false }];
eq(D.chooseAccount(acc, { filterEmail: 'b@x', lastUsed: 'exchange-a' }), 'exchange-b', 'filtered account wins');
eq(D.chooseAccount(acc, { cursorEmail: 'b@x', lastUsed: 'exchange-a' }), 'exchange-b', 'then the cursor event');
eq(D.chooseAccount(acc, { defaultAccount: 'exchange-b', lastUsed: 'exchange-a' }), 'exchange-b', 'then the pinned default');
eq(D.chooseAccount(acc, { lastUsed: 'exchange-b' }), 'exchange-b', 'then last used');
eq(D.chooseAccount(acc, { filterEmail: 'g@x' }), 'gmail-g', 'a gmail filter creates on the Google account (OL-021 track C)');
eq(D.chooseAccount(acc, { lastUsed: 'exchange-c' }), 'gmail-g', 'signed-out last-used is skipped; the first creatable is the Google account now');
eq(D.chooseAccount([{ id: 'imap-x', provider: 'imap', email: 'i@x', signed_in: true }], {}), '', 'nothing can create');
eq(D.cycleAccount(acc, 'exchange-a', 1), 'exchange-b', 'cycle');
eq(D.cycleAccount(acc, 'exchange-b', 1), 'gmail-g', 'wraps through the Google account');

// pending marks
const item = { id: 'co-1', account_id: 'exchange-a', op: 'create', state: 'pending', subject: 'Plan', start: '2026-10-28T10:00', end: '2026-10-28T10:30', all_day: false, error: null };
const p = D.pendingRaw(item, acc);
eq([p.pending, p.failed, p.account_email, p.start], [true, false, 'a@x', '2026-10-28T10:00'], 'pending item');
eq(D.pendingRaw(Object.assign({}, item, { state: 'sent' }), acc), null, 'sent items are not drawn');
eq(D.pendingRaw(Object.assign({}, item, { state: 'cancelled' }), acc), null, 'cancelled items are not drawn');
const f = D.pendingRaw(Object.assign({}, item, { state: 'failed', error: 'nope' }), acc);
eq([f.failed, f.pending, f.error], [true, false, 'nope'], 'failed item stays visible with its reason');
eq([D.pendingMark(p), D.pendingMark(f), D.pendingMark({})], ['\u23f2 ', '\u2717 ', ''], 'marks');

// ---- phase 3 ----
const acct = { id: 'exchange-a', provider: 'exchange', email: 'me@x.org', signed_in: true };
const gmail = { id: 'gmail-g', provider: 'gmail', email: 'g@x.org', signed_in: true };
const req = (email, name) => ({ email, name });

// attendee rows: required first, optional second, own address dropped, duplicates keep the first row, bad text reported
let al = D.attendeeList([req('a@x.org', 'Ann'), req('ME@x.org', 'Me')], [req('A@x.org'), req('bad'), req('b@x.org')], [{ email: 'room@x.org', role: 'resource' }], 'me@x.org');
eq(al.list, [{ name: 'Ann', email: 'a@x.org', role: 'required' }, { name: '', email: 'b@x.org', role: 'optional' }, { name: '', email: 'room@x.org', role: 'resource' }], 'attendee rows');
eq([al.own, al.bad], [true, ['bad']], 'own address and bad text reported, not sent');
eq(D.inviteeCount(al.list), 2, 'rooms are not invitees');
eq(D.attendeeList([], [], [], '').list, [], 'no attendees is an empty list');

eq(D.reachLine(0, 'create'), 'Personal entry. No invitations.', 'create, personal');
eq(D.reachLine(1, 'create'), '1 person will be invited when the undo window ends.', 'create, one person');
eq(D.reachLine(3, 'edit'), 'Invitations with the update go out to 3 people when the undo window ends.', 'edit reaches N');
eq(D.reachLine(0, 'edit'), 'No attendees: nobody is told about the change.', 'edit, nobody');

eq(D.mergePeople([{ name: 'Ann', email: 'A@x.org' }], [{ name: 'Ann again', email: 'a@x.org' }, { name: 'No addr', email: '' }, { email: 'c@x.org' }]),
   [{ name: 'Ann', email: 'A@x.org' }, { name: '', email: 'c@x.org' }], 'merge keeps the first, drops blanks');

// state: attendee order is not a change; subject is
const w1 = { ok: true, start: new Date(2026, 9, 28, 10, 0), end: new Date(2026, 9, 28, 10, 30), allDay: false };
const base = { subject: 'Plan', notes: 'n', when: w1, location: 'Room 4B', reminder: 15, showAs: 'busy',
               attendees: [{ name: 'B', email: 'b@x.org', role: 'optional' }, { name: 'A', email: 'a@x.org', role: 'required' }] };
const s0 = D.stateOf(base);
eq(s0.attendees.map(a => a.email), ['a@x.org', 'b@x.org'], 'attendees sorted');
eq(D.changedKeys(s0, D.stateOf(Object.assign({}, base, { attendees: base.attendees.slice().reverse() }))), [], 'reordered attendees are no change');
eq(D.changedKeys(s0, D.stateOf(Object.assign({}, base, { subject: 'Plan B' }))), ['subject'], 'subject change');
eq(D.changedKeys(s0, D.stateOf(Object.assign({}, base, { reminder: null, when: { ok: true, start: w1.start, end: new Date(2026, 9, 28, 11, 0), allDay: false } }))),
   ['end', 'reminder_minutes'], 'end and reminder changes');
eq(D.stateOf(Object.assign({}, base, { reminder: null })).reminder_minutes, null, 'no reminder is null');

// recurring and refusals
eq([D.isRecurring({ event_type: 'occurrence', series_master_id: 'm1' }), D.isRecurring({ event_type: 'exception', series_master_id: 'm1' }),
    D.isRecurring({ event_type: 'singleInstance', series_master_id: '' }), D.isRecurring({ event_type: 'singleInstance' })],
   [true, true, false, false], 'recurring kinds');
eq(D.editRefusal({ pending: true }, acct), 'That event is not saved yet', 'pending row refused');
eq(D.editRefusal({}, gmail), 'Only the organiser can change this meeting. x declines it.', 'google calendars are editable now; a non-organiser is still refused');
eq(D.editRefusal({}, null), 'This calendar is not connected any more', 'unknown account refused');
eq(D.editRefusal({ is_organizer: false, organizer_email: 'boss@x.org' }, acct), 'Only the organiser can change this meeting. x declines it.', 'attendee cannot edit');
eq(D.editRefusal({ is_organizer: true }, acct), '', 'organiser can edit');
eq(D.editRefusal({ is_organizer: true, organizer_email: 'me@x.org' }, acct), '', 'personal entry (I organise it) can edit');
eq(D.editRefusal({}, acct), 'Only the organiser can change this meeting. x declines it.', 'no organiser info: refused');

// x: delete / cancel / decline
const att = [{ email: 'a@x.org', role: 'required' }];
eq(D.removeKind({ is_organizer: true, attendees: [] }, acct), { kind: 'delete' }, 'personal entry is deleted');
eq(D.removeKind({ is_organizer: true, attendees: att }, acct), { kind: 'cancel' }, 'meeting you organise is cancelled');
eq(D.removeKind({ is_organizer: true, attendees: [{ email: 'room@x.org', role: 'resource' }] }, acct), { kind: 'delete' }, 'rooms are not invitees');
eq(D.removeKind({ is_organizer: false, organizer_email: 'boss@x.org', attendees: att }, acct), { kind: 'decline' }, 'meeting you attend is declined');
eq(D.removeKind({ is_organizer: false, organizer_email: '' }, acct), { error: 'Not your event: only its organiser can delete it' }, 'not yours, not deleted');
eq(D.removeKind({ is_organizer: true, attendees: [] }, gmail).kind, 'delete', 'google personal entries can be removed (OL-021 track C)');

// paths and messages
eq(D.changePath('update', 'id 1', 'series', '', false), '/calendar/event?id=id%201&scope=series', 'patch path');
eq(D.changePath('update', 'x', 'occurrence', '', true), '/calendar/event?id=x&scope=occurrence&dry_run=1', 'patch dry-run path');
eq(D.changePath('cancel', 'x', 'occurrence', 'cancel', false), '/calendar/event/cancel?id=x&scope=occurrence&kind=cancel', 'cancel path');
eq(D.changePath('delete', 'x', 'bogus', 'delete', true), '/calendar/event/cancel?id=x&scope=occurrence&kind=delete&dry_run=1', 'unknown scope falls to occurrence');
eq(D.changeMessage('cancel', 1, false), 'Cancellation queued for 1 person', 'cancel message');
eq(D.changeMessage('update', 0, true), 'Change queued on the whole series (personal)', 'update message, personal series');
eq(D.removeLine('cancel', -1), 'Counting who is invited\u2026', 'counting');
eq(D.removeLine('cancel', 4), 'The cancellation goes to 4 people when the undo window ends.', 'cancel line counts');
eq(D.removeLine('decline', 4), 'Your decline goes to the organiser only. Nothing else is sent.', 'decline line');
eq(D.removeLine('delete', -1), 'Removed from your calendar when the undo window ends. Nobody is told.', 'delete line');

// pending overlay on cached rows
const rows = [
  { id: 'e1', subject: 'Standup', start: '2026-10-28T09:00:00', end: '2026-10-28T09:30:00', location: '', series_master_id: '' },
  { id: 'e2', subject: 'Weekly', start: '2026-10-29T09:00:00', end: '2026-10-29T10:00:00', location: 'Room', series_master_id: 'm1' },
  { id: 'e3', subject: 'Other', start: '2026-10-30T09:00:00', end: '2026-10-30T10:00:00', location: '', series_master_id: '' },
];
const items = [
  { id: 'o1', op: 'update', state: 'pending', target: 'e1', series: false, subject: 'Standup moved', start: '2026-10-28T11:00:00', end: '2026-10-28T11:30:00', all_day: false, location: 'Room 2' },
  { id: 'o2', op: 'delete', state: 'failed', target: 'm1', series: true, error: 'nope' },
  { id: 'o3', op: 'create', state: 'pending', target: '' },
  { id: 'o4', op: 'cancel', state: 'sent', target: 'e3', series: false },
];
const ov = D.applyChanges(rows, items);
eq(ov[0].subject, '\u23f2 Standup moved', 'pending edit marked and retitled');
eq([ov[0].start, ov[0].end, ov[0].location, ov[0].pending], ['2026-10-28T11:00:00', '2026-10-28T11:30:00', 'Room 2', true], 'edit moves the row');
eq([ov[1].subject, ov[1].failed, ov[1].pendingOp], ['\u2717 Weekly', true, 'delete'], 'failed removal on a series row (master id)');
eq(ov[2].subject, 'Other', 'sent change is not drawn');
eq(D.applyChanges(rows, []), rows, 'no changes: rows unchanged');
eq(rows[0].subject, 'Standup', 'cache rows are not mutated');

// buildPatch: whole state, attendees always, nothing changed refused
const cur = Object.assign({}, base, { reminder: null, attendees: [] });
const orig = D.stateOf(base);
const changed = D.changedKeys(orig, D.stateOf(cur));
let pb = D.buildPatch(Object.assign({}, cur, { accountId: 'exchange-a', changed, originalLocation: 'Room 4B', location: '' }));
eq(pb.ok, false, 'a cleared place is refused (not supported yet)');
pb = D.buildPatch(Object.assign({}, cur, { accountId: 'exchange-a', changed, originalLocation: '', location: '' }));
eq(pb.ok, true, 'patch ok');
eq(pb.body, { subject: 'Plan', body: 'n', start: '2026-10-28T10:00:00', end: '2026-10-28T10:30:00', all_day: false,
              show_as: 'busy', attendees: [], account_id: 'exchange-a' }, 'full state, empty attendee list sent, reminder off');
eq(D.buildPatch(Object.assign({}, base, { accountId: 'exchange-a', changed: [] })), { ok: false, nothing: true, error: 'Nothing changed yet' }, 'nothing changed is not sent');
eq(D.buildPatch(Object.assign({}, base, { accountId: '', changed: ['subject'] })).ok, false, 'no account');
eq(D.buildPatch(Object.assign({}, base, { accountId: 'x', changed: ['subject'], subject: '  ' })), { ok: false, error: 'a title is needed' }, 'title needed');
console.log('draft tests ok (' + n + ' checks)');
