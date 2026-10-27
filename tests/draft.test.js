const fs = require('fs');
const src = fs.readFileSync(__dirname + '/../qml/calendar/draft.js', 'utf8').replace(/^\.pragma.*$/m, '');
const D = new Function(src + ';return {stamp,buildRequest,conflicts,canCreate,chooseAccount,cycleAccount,pendingRaw,pendingMark}')();
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
eq(D.chooseAccount(acc, { filterEmail: 'g@x' }), 'exchange-a', 'a gmail filter cannot create yet: falls through to the first creatable');
eq(D.chooseAccount(acc, { lastUsed: 'exchange-c' }), 'exchange-a', 'signed-out last-used is skipped');
eq(D.chooseAccount([acc[0]], {}), '', 'nothing can create');
eq(D.cycleAccount(acc, 'exchange-a', 1), 'exchange-b', 'cycle');
eq(D.cycleAccount(acc, 'exchange-b', 1), 'exchange-a', 'wraps');

// pending marks
const item = { id: 'co-1', account_id: 'exchange-a', op: 'create', state: 'pending', subject: 'Plan', start: '2026-10-28T10:00', end: '2026-10-28T10:30', all_day: false, error: null };
const p = D.pendingRaw(item, acc);
eq([p.pending, p.failed, p.account_email, p.start], [true, false, 'a@x', '2026-10-28T10:00'], 'pending item');
eq(D.pendingRaw(Object.assign({}, item, { state: 'sent' }), acc), null, 'sent items are not drawn');
eq(D.pendingRaw(Object.assign({}, item, { state: 'cancelled' }), acc), null, 'cancelled items are not drawn');
const f = D.pendingRaw(Object.assign({}, item, { state: 'failed', error: 'nope' }), acc);
eq([f.failed, f.pending, f.error], [true, false, 'nope'], 'failed item stays visible with its reason');
eq([D.pendingMark(p), D.pendingMark(f), D.pendingMark({})], ['\u23f2 ', '\u2717 ', ''], 'marks');
console.log('draft tests ok (' + n + ' checks)');
