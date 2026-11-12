// Create-from-mail helper (qml/calendar/frommail.js). Run: node tests/frommail.test.js
const fs = require('fs');
const path = require('path');
const load = (f) => new Function(fs.readFileSync(path.join(__dirname, '..', 'qml', 'calendar', f), 'utf8').replace('.pragma library', '') + ';return {' +
  (f === 'frommail.js' ? 'cleanSubject,parseAddresses,excerpt,candidates,nextFreeHour,suggestTime,prefill' : 'parseWhen') + '}')();
const FM = load('frommail.js');
const W = load('when.js');

const eq = (a, b, m) => { const sa = JSON.stringify(a), sb = JSON.stringify(b); if (sa !== sb) { console.error('FAIL', m, sa, '!=', sb); process.exit(1) } };
const ok = (c, m) => { if (!c) { console.error('FAIL', m); process.exit(1) } };

// Tue 27 Oct 2026 09:00 local.
const NOW = new Date(2026, 9, 27, 9, 0);

// subjects
eq(FM.cleanSubject('Re: Fwd: Lunch on Friday'), 'Lunch on Friday', 're/fwd');
eq(FM.cleanSubject('AW: Angebot'), 'Angebot', 'aw');
eq(FM.cleanSubject('RE[2]: budget'), 'budget', 'bracketed count');
eq(FM.cleanSubject('Relax: now'), 'Relax: now', 'word starting with re is kept');
eq(FM.cleanSubject(''), '', 'empty');

// addresses
eq(FM.parseAddresses('Ann <ann@x.com>, bob@y.org; "Smith, Jo" <jo@z.net>'),
   [{ name: 'Ann', email: 'ann@x.com' }, { name: '', email: 'bob@y.org' }, { name: 'Smith, Jo', email: 'jo@z.net' }], 'list');
eq(FM.parseAddresses('ANN@X.COM, ann@x.com <Ann@x.com>, not-an-address'), [{ name: '', email: 'ANN@X.COM' }], 'dedupe and junk');
eq(FM.parseAddresses(''), [], 'empty list');
eq(FM.parseAddresses('mailto:q@w.io'), [{ name: '', email: 'q@w.io' }], 'mailto');

// excerpt
eq(FM.excerpt('  a \n\n b  ', 600), 'a b', 'collapse');
ok(FM.excerpt('word '.repeat(200), 50).length <= 51, 'excerpt length');
ok(FM.excerpt('word '.repeat(200), 50).endsWith('…'), 'excerpt ellipsis');

// time: text phrase wins when the parser reads a future timed phrase
const t1 = FM.suggestTime(['Can we do tomorrow 3pm?'], { now: NOW, parseWhen: W.parseWhen, defaultMinutes: 30 });
eq(t1.start.getTime(), new Date(2026, 9, 28, 15, 0).getTime(), 'tomorrow 3pm start');
eq(t1.end.getTime() - t1.start.getTime(), 30 * 60000, 'default length');

// a past phrase is not used: falls back to the next free hour
const t2 = FM.suggestTime(['meeting yesterday 8am'], { now: NOW, parseWhen: W.parseWhen, defaultMinutes: 30 });
eq(t2.start.getTime(), new Date(2026, 9, 27, 9, 30).getTime(), 'past phrase -> next half hour');
eq(t2.source, 'next free half hour', 'source fallback');

// no date at all: next whole hour
const t3 = FM.suggestTime(['no time here'], { now: NOW, parseWhen: W.parseWhen, defaultMinutes: 45 });
eq(t3.start.getTime(), new Date(2026, 9, 27, 9, 30).getTime(), 'next half hour');
eq(t3.end.getTime() - t3.start.getTime(), 45 * 60000, 'length 45');

// busy slots are skipped
const busy = [{ start: new Date(2026, 9, 27, 9, 30), end: new Date(2026, 9, 27, 10, 0) }, { start: new Date(2026, 9, 27, 10, 0), end: new Date(2026, 9, 27, 11, 0) }, { start: new Date(2026, 9, 27, 11, 0), end: new Date(2026, 9, 27, 11, 30) }];
const t4 = FM.nextFreeHour(NOW, busy, 30);
eq(t4.start.getTime(), new Date(2026, 9, 27, 11, 30).getTime(), 'skips busy 9:30-11:30 in half-hour steps');

// a time phrase that overlaps nothing but the parser fails: no crash
const t5 = FM.suggestTime(['at 25:99 maybe'], { now: NOW, parseWhen: () => { throw new Error('boom') } });
eq(t5.source, 'next free half hour', 'parser throws -> fallback');

// full prefill
const msg = {
  subject: 'Re: Q4 review', from_name: 'Dana Lee', from_email: 'dana@acme.com', account_id: 'graph-abc123',
  to_text: 'Me <me@work.com>, eve@acme.com', cc_text: 'eve@acme.com, Frank <frank@acme.com>, me@work.com',
  body_preview: 'Hi all, can we meet on Thursday at 2pm to go through the numbers? Thanks, Dana',
};
const p = FM.prefill(msg, { now: NOW, ownEmails: ['me@work.com'], parseWhen: W.parseWhen, defaultMinutes: 30, defaultReminder: 10, busy: [] });
eq(p.subject, 'Q4 review', 'subject');
eq(p.accountId, 'graph-abc123', 'account from message');
eq(p.required, [{ name: 'Dana Lee', email: 'dana@acme.com' }, { name: '', email: 'eve@acme.com' }], 'required: sender then To, own removed');
eq(p.optional, [{ name: 'Frank', email: 'frank@acme.com' }], 'optional: Cc without duplicates, own removed');
eq(p.start.getTime(), new Date(2026, 9, 29, 14, 0).getTime(), 'Thursday 2pm (Thu 29 Oct)');
ok(p.notes.startsWith('Hi all, can we meet'), 'notes excerpt');
eq(p.restore.reminder, '10', 'reminder string');
eq(p.restore.showAs, 'busy', 'show as');
eq(p.restore.base.start.getTime(), p.start.getTime(), 'restore base');
eq(p.restore.required, [{ email: 'dana@acme.com', name: 'Dana Lee' }, { email: 'eve@acme.com', name: '' }], 'restore required');

// body that was loaded wins over the preview; no sender, no to: empty people
const p2 = FM.prefill({ subject: 'x' }, { now: NOW, ownEmails: [], parseWhen: W.parseWhen, defaultMinutes: 30, body: 'Stand-up tomorrow 9:30am' });
eq(p2.required, [], 'no people');
eq(p2.start.getTime(), new Date(2026, 9, 28, 9, 30).getTime(), 'body time');
eq(p2.accountId, '', 'no account');

// nothing in the message and no parser: still an editor prefill
const p3 = FM.prefill(undefined, { now: NOW });
eq(p3.subject, '', 'empty message subject');
eq(p3.start.getTime(), new Date(2026, 9, 27, 9, 30).getTime(), 'empty message -> next half hour');

console.log('ok frommail');
