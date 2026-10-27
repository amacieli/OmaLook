const fs = require('fs');
const path = require('path');
const src = fs.readFileSync(path.join(__dirname, '..', 'qml', 'calendar', 'when.js'), 'utf8').replace('.pragma library', '');
const W = new Function(src + ';return {parseWhen,nudge,formatSummary}')();

// Tue 27 Oct 2026 09:00 local. Every case below is relative to this.
const NOW = new Date(2026, 9, 27, 9, 0);
// Current value 14:00-15:30 (90 min) on Tue 27 Oct.
const BASE90 = { base: { start: new Date(2026, 9, 27, 14, 0), end: new Date(2026, 9, 27, 15, 30) } };
// Current value is an all-day event (Tue 27 Oct, 1440 min).
const BASE_ALLDAY = { base: { start: new Date(2026, 9, 27, 0, 0), end: new Date(2026, 9, 28, 0, 0) } };

let checks = 0;
function fail(msg) { console.error('FAIL', msg); process.exit(1); }
function eq(a, b, m) {
    checks++;
    if (a !== b) fail(m + ' expected ' + JSON.stringify(b) + ' got ' + JSON.stringify(a));
}
function pad(n) { return n < 10 ? '0' + n : '' + n; }
function ymdhm(d) {
    return d.getFullYear() + '-' + pad(d.getMonth() + 1) + '-' + pad(d.getDate()) + ' ' + pad(d.getHours()) + ':' + pad(d.getMinutes());
}
function run(text, extra) {
    return W.parseWhen(text, Object.assign({ now: NOW, defaultMinutes: 30, base: null }, extra || {}));
}

// [text, start, end, minutes, allDay?, extra?]   dates as 'YYYY-MM-DD HH:MM'
const OK = [
    ['tomorrow 3pm', '2026-10-28 15:00', '2026-10-28 15:30', 30],
    ['today', '2026-10-27 09:00', '2026-10-27 09:30', 30],
    ['today 14:00 1h', '2026-10-27 14:00', '2026-10-27 15:00', 60],
    ['tue', '2026-10-27 09:00', '2026-10-27 09:30', 30],           // weekday on today = today
    ['mon', '2026-11-02 09:00', '2026-11-02 09:30', 30],           // next Monday on/after today
    ['sat 10am', '2026-10-31 10:00', '2026-10-31 10:30', 30],
    ['fri 10-10:30', '2026-10-30 10:00', '2026-10-30 10:30', 30],
    ['thu 9:15 45m', '2026-10-29 09:15', '2026-10-29 10:00', 45],
    ['next tue 2pm for 45m', '2026-11-03 14:00', '2026-11-03 14:45', 45], // strictly after today
    ['next mon', '2026-11-02 09:00', '2026-11-02 09:30', 30],
    ['next tue', '2026-11-03 09:00', '2026-11-03 09:30', 30],
    ['27 oct 14:00 1h', '2026-10-27 14:00', '2026-10-27 15:00', 60],
    ['oct 27 14:00 1h', '2026-10-27 14:00', '2026-10-27 15:00', 60],
    ['1 nov 9am', '2026-11-01 09:00', '2026-11-01 09:30', 30],     // month rollover
    ['31 dec 23:30 1h', '2026-12-31 23:30', '2027-01-01 00:30', 60], // year and midnight rollover
    ['27 oct 2027 10:00', '2027-10-27 10:00', '2027-10-27 10:30', 30],
    ['3 jan 9am', '2027-01-03 09:00', '2027-01-03 09:30', 30],     // no year: next such date
    ['2026-11-05 10:00', '2026-11-05 10:00', '2026-11-05 10:30', 30],
    ['all day mon', '2026-11-02 00:00', '2026-11-03 00:00', 1440, true],
    ['all day', '2026-10-27 00:00', '2026-10-28 00:00', 1440, true],
    ['allday tomorrow', '2026-10-28 00:00', '2026-10-29 00:00', 1440, true],
    ['2pm-3:30pm', '2026-10-27 14:00', '2026-10-27 15:30', 90],
    ['10:00-11:00', '2026-10-27 10:00', '2026-10-27 11:00', 60],
    ['10 to 10:30', '2026-10-27 10:00', '2026-10-27 10:30', 30],
    ['fri 10 - 10:30', '2026-10-30 10:00', '2026-10-30 10:30', 30],
    ['tomorrow at 9 am', '2026-10-28 09:00', '2026-10-28 09:30', 30],
    ['at 5pm', '2026-10-27 17:00', '2026-10-27 17:30', 30],
    ['17:00', '2026-10-27 17:00', '2026-10-27 17:30', 30],
    ['3pm', '2026-10-27 15:00', '2026-10-27 15:30', 30],          // time only, no base: today
    ['12am', '2026-10-27 00:00', '2026-10-27 00:30', 30],
    ['12pm', '2026-10-27 12:00', '2026-10-27 12:30', 30],
    ['tomorrow, 3pm, 45m', '2026-10-28 15:00', '2026-10-28 15:45', 45],
    ['tomorrow 3pm 1h30m', '2026-10-28 15:00', '2026-10-28 16:30', 90],
    ['tomorrow 15:00 90m', '2026-10-28 15:00', '2026-10-28 16:30', 90],
    ['tomorrow 9:00 2h', '2026-10-28 09:00', '2026-10-28 11:00', 120],
    ['tomorrow', '2026-10-28 09:00', '2026-10-28 09:30', 30],     // date only, no base: 09:00
    // relative edits, base = Tue 27 Oct 14:00-15:30
    ['+30m', '2026-10-27 14:30', '2026-10-27 16:00', 90, false, BASE90],
    ['-1h', '2026-10-27 13:00', '2026-10-27 14:30', 90, false, BASE90],
    ['+1d', '2026-10-28 14:00', '2026-10-28 15:30', 90, false, BASE90],
    ['10:30', '2026-10-27 10:30', '2026-10-27 12:00', 90, false, BASE90],   // keeps date and length
    ['mon', '2026-11-02 14:00', '2026-11-02 15:30', 90, false, BASE90],     // keeps time and length
    ['tomorrow', '2026-10-28 14:00', '2026-10-28 15:30', 90, false, BASE90],
    ['tomorrow 3pm', '2026-10-28 15:00', '2026-10-28 16:30', 90, false, BASE90], // typed time, base length kept
    ['fri 10-10:30', '2026-10-30 10:00', '2026-10-30 10:30', 30, false, BASE90], // typed range wins
    ['1h', '2026-10-27 14:00', '2026-10-27 15:00', 60, false, BASE90],           // typed length wins
    // base that is all-day: its 24h length is not kept
    ['10:30', '2026-10-27 10:30', '2026-10-27 11:00', 30, false, BASE_ALLDAY],
    // defaultMinutes
    ['3pm', '2026-10-27 15:00', '2026-10-27 16:00', 60, false, { defaultMinutes: 60 }],
];

for (const row of OK) {
    const [text, s, e, mins, allDay, extra] = row;
    const r = run(text, extra);
    if (!r.ok) fail('expected ok for ' + JSON.stringify(text) + ': ' + JSON.stringify(r));
    eq(ymdhm(r.start), s, text + ' start');
    eq(ymdhm(r.end), e, text + ' end');
    eq(r.minutes, mins, text + ' minutes');
    eq(r.allDay, !!allDay, text + ' allDay');
}

// Summaries (formatting is part of the contract, the parser must produce these exactly)
eq(run('10:00').summary, 'Tue 27 Oct 10:00 \u2192 10:30 (30m)', 'summary 30m');
eq(run('31 dec 23:30 1h').summary, 'Thu 31 Dec 23:30 \u2192 Fri 1 Jan 2027 00:30 (1h)', 'summary across midnight');
eq(run('all day mon').summary, 'Mon 2 Nov (all day)', 'summary all day');
eq(run('10-12').ok, false, 'bare 10-12 is an error, not a summary');
eq(run('2pm-3:30pm').summary, 'Tue 27 Oct 14:00 \u2192 15:30 (1h30m)', 'summary 1h30m');
eq(run('tomorrow 10:00 2h').summary, 'Wed 28 Oct 10:00 \u2192 12:00 (2h)', 'summary 2h');
eq(W.formatSummary(new Date(2027, 9, 27, 10, 0), new Date(2027, 9, 27, 10, 30), false, NOW),
    'Wed 27 Oct 2027 10:00 \u2192 10:30 (30m)', 'year shown when not current year');

// Errors: ok:false, with an error text and (where useful) the understood part
const ERR = [
    ['', null],
    ['   ', null],
    ['5', null],                       // bare number: never a time
    ['17', null],
    ['13pm', null],
    ['25:00', null],
    ['10:61', null],
    ['10-11', null],                   // both bare
    ['9-10:30', null],                 // bare start, different hour
    ['10-11pm', null],                 // bare start with am/pm on the end
    ['10am-11', null],                 // bare end
    ['10:00-', null],
    ['10-', null],
    ['11:00-10:00', null],             // end before start
    ['10-10:30pm', null],              // bare start, end has am/pm
    ['fri 10-10:30 1h', null],         // range and length
    ['tomorrow 3pm 4pm', null],        // two times
    ['tomorrow fri', null],            // two dates
    ['all day 3pm', null],
    ['all day 1h', null],
    ['next', null],
    ['next 5', null],
    ['foo', null],
    ['31 feb', null],
    ['2026-13-01 10:00', null],
    ['0 oct', null],
    ['0m', null],
    ['tomorrow 1h 2h', null],
    ['+30m', null],                    // no base to shift
    ['+30m tomorrow', null, BASE90],   // shift is on its own
    ['27', null],
    ['oct', null],
    ['2026', null],
    ['tomorrow 5', 'Wed 28 Oct', null],// understood part shows the date
];
for (const row of ERR) {
    const [text, understood, extra] = row;
    const r = run(text, extra ? extra : null);
    eq(r.ok, false, 'error for ' + JSON.stringify(text));
    if (typeof r.error !== 'string' || r.error.length === 0) fail('no error text for ' + JSON.stringify(text));
    if (typeof r.understood !== 'string') fail('no understood field for ' + JSON.stringify(text));
    if (understood) eq(r.understood.indexOf(understood) >= 0, true, 'understood for ' + text + ' was ' + JSON.stringify(r.understood));
}
// The bare-number message names the token
eq(run('5').error.indexOf("'5'") >= 0, true, 'bare 5 error names the token');
eq(run('tomorrow 5').error.indexOf("'5'") >= 0, true, 'tomorrow 5 error names the token');

// nudge
const V = { start: new Date(2026, 9, 27, 10, 0), end: new Date(2026, 9, 27, 10, 30) };
function ns(v) { return ymdhm(v.start) + '|' + ymdhm(v.end); }
eq(ns(W.nudge(V, 'start', 1)), '2026-10-27 10:15|2026-10-27 10:45', 'nudge start up');
eq(ns(W.nudge(V, 'start', -1)), '2026-10-27 09:45|2026-10-27 10:15', 'nudge start down');
eq(ns(W.nudge(V, 'day', 1)), '2026-10-28 10:00|2026-10-28 10:30', 'shift day forward');
eq(ns(W.nudge(V, 'day', -1)), '2026-10-26 10:00|2026-10-26 10:30', 'shift day back');
eq(ns(W.nudge(V, 'length', 1)), '2026-10-27 10:00|2026-10-27 10:45', 'length up');
eq(ns(W.nudge(V, 'length', -1)), '2026-10-27 10:00|2026-10-27 10:15', 'length down to 15');
const V15 = { start: new Date(2026, 9, 27, 10, 0), end: new Date(2026, 9, 27, 10, 15) };
eq(ns(W.nudge(V15, 'length', -1)), '2026-10-27 10:00|2026-10-27 10:15', 'length minimum 15');
eq(ns(W.nudge(V15, 'length', 1)), '2026-10-27 10:00|2026-10-27 10:30', 'length up from 15');
const VA = { start: new Date(2026, 9, 27, 0, 0), end: new Date(2026, 9, 28, 0, 0), allDay: true };
eq(ns(W.nudge(VA, 'start', 1)), '2026-10-27 00:00|2026-10-28 00:00', 'all-day start nudge is a no-op');
eq(ns(W.nudge(VA, 'day', 1)), '2026-10-28 00:00|2026-10-29 00:00', 'all-day day nudge');
eq(W.nudge(VA, 'day', 1).allDay, true, 'all-day kept');

console.log('when tests ok (' + checks + ' checks, ' + OK.length + ' parse cases, ' + ERR.length + ' error cases)');
