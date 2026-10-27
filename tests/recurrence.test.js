const fs = require('fs');
const path = require('path');
const src = fs.readFileSync(path.join(__dirname, '..', 'qml', 'calendar', 'recurrence.js'), 'utf8').replace('.pragma library', '');
const R = new Function(src + ';return {parseRepeat,listChoices}')();

// Tue 27 Oct 2026 10:00 local: the event start for most cases.
const START = new Date(2026, 9, 27, 10, 0);
const OPT = { start: START, defaultSeriesMonths: 12 };

let checks = 0;
function fail(msg) { console.error('FAIL', msg); process.exit(1); }
function canon(v) {
    if (v === null || typeof v !== 'object') return v;
    if (Array.isArray(v)) return v.map(canon);
    const o = {};
    Object.keys(v).sort().forEach(function (k) { o[k] = canon(v[k]); });
    return o;
}
function eq(a, b, m) {
    checks++;
    const ja = JSON.stringify(canon(a)), jb = JSON.stringify(canon(b));
    if (ja !== jb) fail(m + '\n   expected ' + jb + '\n   got      ' + ja);
}
function pad(n) { return n < 10 ? '0' + n : '' + n; }
function ymd(d) { return d.getFullYear() + '-' + pad(d.getMonth() + 1) + '-' + pad(d.getDate()); }
function run(text, extra) {
    return R.parseRepeat(text, Object.assign({ start: START, defaultSeriesMonths: 12 }, extra || {}));
}

// [text, ctx overrides, pattern, range, sentence, next (YYYY-MM-DD), explicitEnd]
const OK = [
    ['daily for 10', null,
        { type: 'daily', interval: 1 }, { type: 'numbered', numberOfOccurrences: 10 },
        'Daily, 10 occurrences', ['2026-10-28', '2026-10-29', '2026-10-30'], true],
    ['daily for 10 times', null,
        { type: 'daily', interval: 1 }, { type: 'numbered', numberOfOccurrences: 10 },
        'Daily, 10 occurrences', ['2026-10-28', '2026-10-29', '2026-10-30'], true],
    ['daily for 1', null,
        { type: 'daily', interval: 1 }, { type: 'numbered', numberOfOccurrences: 1 },
        'Daily, 1 occurrence', [], true],
    ['daily for 2', null,
        { type: 'daily', interval: 1 }, { type: 'numbered', numberOfOccurrences: 2 },
        'Daily, 2 occurrences', ['2026-10-28'], true],
    ['daily until 29 dec', null,
        { type: 'daily', interval: 1 }, { type: 'endDate', endDate: '2026-12-29' },
        'Daily until 29 Dec 2026', ['2026-10-28', '2026-10-29', '2026-10-30'], true],
    ['daily until 2 nov', null,
        { type: 'daily', interval: 1 }, { type: 'endDate', endDate: '2026-11-02' },
        'Daily until 2 Nov 2026', ['2026-10-28', '2026-10-29', '2026-10-30'], true],
    ['daily until 29 dec 2027', null,
        { type: 'daily', interval: 1 }, { type: 'endDate', endDate: '2027-12-29' },
        'Daily until 29 Dec 2027', ['2026-10-28', '2026-10-29', '2026-10-30'], true],
    ['daily until 2027-01-05', null,
        { type: 'daily', interval: 1 }, { type: 'endDate', endDate: '2027-01-05' },
        'Daily until 5 Jan 2027', ['2026-10-28', '2026-10-29', '2026-10-30'], true],
    ['daily', null,
        { type: 'daily', interval: 1 }, { type: 'endDate', endDate: '2027-10-27' },
        'Daily until 27 Oct 2027 (default: 12 months)', ['2026-10-28', '2026-10-29', '2026-10-30'], false],
    ['daily never', null,
        { type: 'daily', interval: 1 }, { type: 'noEnd' },
        'Daily, no end date', ['2026-10-28', '2026-10-29', '2026-10-30'], true],
    ['every 3 days', null,
        { type: 'daily', interval: 3 }, { type: 'endDate', endDate: '2027-10-27' },
        'Every 3 days until 27 Oct 2027 (default: 12 months)', ['2026-10-30', '2026-11-02', '2026-11-05'], false],
    ['weekdays', null,
        { type: 'weekly', interval: 1, daysOfWeek: ['monday', 'tuesday', 'wednesday', 'thursday', 'friday'] },
        { type: 'endDate', endDate: '2027-10-27' },
        'Weekdays until 27 Oct 2027 (default: 12 months)', ['2026-10-28', '2026-10-29', '2026-10-30'], false],
    ['weekly', null,
        { type: 'weekly', interval: 1, daysOfWeek: ['tuesday'] }, { type: 'endDate', endDate: '2027-10-27' },
        'Weekly on Tue until 27 Oct 2027 (default: 12 months)', ['2026-11-03', '2026-11-10', '2026-11-17'], false],
    ['weekly on tue', null,
        { type: 'weekly', interval: 1, daysOfWeek: ['tuesday'] }, { type: 'endDate', endDate: '2027-10-27' },
        'Weekly on Tue until 27 Oct 2027 (default: 12 months)', ['2026-11-03', '2026-11-10', '2026-11-17'], false],
    ['weekly on tue,tue', null,
        { type: 'weekly', interval: 1, daysOfWeek: ['tuesday'] }, { type: 'endDate', endDate: '2027-10-27' },
        'Weekly on Tue until 27 Oct 2027 (default: 12 months)', ['2026-11-03', '2026-11-10', '2026-11-17'], false],
    ['weekly on tue,thu', null,
        { type: 'weekly', interval: 1, daysOfWeek: ['tuesday', 'thursday'] }, { type: 'endDate', endDate: '2027-10-27' },
        'Weekly on Tue, Thu until 27 Oct 2027 (default: 12 months)', ['2026-10-29', '2026-11-03', '2026-11-05'], false],
    ['every 2 weeks on tue,thu', null,
        { type: 'weekly', interval: 2, daysOfWeek: ['tuesday', 'thursday'] }, { type: 'endDate', endDate: '2027-10-27' },
        'Every 2 weeks on Tue, Thu until 27 Oct 2027 (default: 12 months)', ['2026-10-29', '2026-11-10', '2026-11-12'], false],
    ['every 2 weeks', null,
        { type: 'weekly', interval: 2, daysOfWeek: ['tuesday'] }, { type: 'endDate', endDate: '2027-10-27' },
        'Every 2 weeks on Tue until 27 Oct 2027 (default: 12 months)', ['2026-11-10', '2026-11-24', '2026-12-08'], false],
    ['weekly until 29 dec', null,
        { type: 'weekly', interval: 1, daysOfWeek: ['tuesday'] }, { type: 'endDate', endDate: '2026-12-29' },
        'Weekly on Tue until 29 Dec 2026', ['2026-11-03', '2026-11-10', '2026-11-17'], true],
    ['weekly until 10 nov', null,
        { type: 'weekly', interval: 1, daysOfWeek: ['tuesday'] }, { type: 'endDate', endDate: '2026-11-10' },
        'Weekly on Tue until 10 Nov 2026', ['2026-11-03', '2026-11-10'], true],
    ['weekly never', null,
        { type: 'weekly', interval: 1, daysOfWeek: ['tuesday'] }, { type: 'noEnd' },
        'Weekly on Tue, no end date', ['2026-11-03', '2026-11-10', '2026-11-17'], true],
    ['monthly', null,
        { type: 'absoluteMonthly', interval: 1, dayOfMonth: 27 }, { type: 'endDate', endDate: '2027-10-27' },
        'Monthly on the 27th until 27 Oct 2027 (default: 12 months)', ['2026-11-27', '2026-12-27', '2027-01-27'], false],
    ['monthly 27', null,
        { type: 'absoluteMonthly', interval: 1, dayOfMonth: 27 }, { type: 'endDate', endDate: '2027-10-27' },
        'Monthly on the 27th until 27 Oct 2027 (default: 12 months)', ['2026-11-27', '2026-12-27', '2027-01-27'], false],
    ['monthly 4th tue', null,
        { type: 'relativeMonthly', interval: 1, index: 'fourth', daysOfWeek: ['tuesday'] }, { type: 'endDate', endDate: '2027-10-27' },
        'Monthly on the 4th Tue until 27 Oct 2027 (default: 12 months)', ['2026-11-24', '2026-12-22', '2027-01-26'], false],
    ['monthly last tue', null,
        { type: 'relativeMonthly', interval: 1, index: 'last', daysOfWeek: ['tuesday'] }, { type: 'endDate', endDate: '2027-10-27' },
        'Monthly on the last Tue until 27 Oct 2027 (default: 12 months)', ['2026-11-24', '2026-12-29', '2027-01-26'], false],
    ['monthly on the 4th tue', null,
        { type: 'relativeMonthly', interval: 1, index: 'fourth', daysOfWeek: ['tuesday'] }, { type: 'endDate', endDate: '2027-10-27' },
        'Monthly on the 4th Tue until 27 Oct 2027 (default: 12 months)', ['2026-11-24', '2026-12-22', '2027-01-26'], false],
    ['monthly 4th tuesday until 26 jan 2027', null,
        { type: 'relativeMonthly', interval: 1, index: 'fourth', daysOfWeek: ['tuesday'] }, { type: 'endDate', endDate: '2027-01-26' },
        'Monthly on the 4th Tue until 26 Jan 2027', ['2026-11-24', '2026-12-22', '2027-01-26'], true],
    // Dates of the monthly-31st pattern are in the test below (start on the 31st)
    ['weekly on tue,thu for 3', null,
        { type: 'weekly', interval: 1, daysOfWeek: ['tuesday', 'thursday'] }, { type: 'numbered', numberOfOccurrences: 3 },
        'Weekly on Tue, Thu, 3 occurrences', ['2026-10-29', '2026-11-03'], true],
    ['none', null, null, null, 'Does not repeat', [], false],
    ['', null, null, null, 'Does not repeat', [], false],
];

for (const row of OK) {
    const [text, extra, pattern, range, sentence, next, explicit] = row;
    const r = run(text, extra);
    if (!r.ok) fail('expected ok for ' + JSON.stringify(text) + ': ' + JSON.stringify(r));
    eq(r.pattern, pattern, text + ' pattern');
    eq(r.range, range, text + ' range');
    eq(r.sentence, sentence, text + ' sentence');
    eq(r.next.map(ymd), next, text + ' next');
    eq(r.explicitEnd, explicit, text + ' explicitEnd');
    if (pattern !== null && r.next.length !== next.length) fail('next length ' + text);
}

// Default end follows defaultSeriesMonths; 0 means no end
eq(run('daily', { defaultSeriesMonths: 0 }).range, { type: 'noEnd' }, 'default 0 = no end');
eq(run('daily', { defaultSeriesMonths: 0 }).sentence, 'Daily, no end date', 'default 0 sentence');
eq(run('daily', { defaultSeriesMonths: 1 }).sentence, 'Daily until 27 Nov 2026 (default: 1 month)', 'default 1 month');

// 'until' without a year: next such date on or after the start (here 29 Dec is already past for 30 Dec 2026)
eq(run('daily until 29 dec', { start: new Date(2026, 11, 30, 10, 0) }).range,
    { type: 'endDate', endDate: '2027-12-29' }, 'until rolls to next year');
eq(run('daily until 29 dec', { start: new Date(2026, 11, 29, 10, 0) }).range,
    { type: 'endDate', endDate: '2026-12-29' }, 'until same day is on/after start');

// The 31st: months without a 31st are skipped (Graph and Outlook behaviour)
const START31 = new Date(2026, 9, 31, 10, 0);   // Sat 31 Oct 2026
const r31 = run('monthly', { start: START31 });
eq(r31.pattern, { type: 'absoluteMonthly', interval: 1, dayOfMonth: 31 }, 'monthly 31 pattern');
eq(r31.next.map(ymd), ['2026-12-31', '2027-01-31', '2027-03-31'], 'monthly on 31st skips Nov, Feb');

// Last Friday, start on the last Friday of October (Fri 30 Oct 2026)
const START_FRI = new Date(2026, 9, 30, 10, 0);
eq(run('monthly last fri', { start: START_FRI }).next.map(ymd),
    ['2026-11-27', '2026-12-25', '2027-01-29'], 'monthly last fri');
eq(run('monthly last fri', { start: START_FRI }).pattern,
    { type: 'relativeMonthly', interval: 1, index: 'last', daysOfWeek: ['friday'] }, 'monthly last fri pattern');

// Weekdays from a Monday start: the start is itself the first occurrence and is not in 'next'
eq(run('weekdays', { start: new Date(2026, 9, 26, 9, 0) }).next.map(ymd),
    ['2026-10-27', '2026-10-28', '2026-10-29'], 'weekdays from Monday');

// A Monday-start weekly on Mon: 'next' skips the start
eq(run('weekly', { start: new Date(2026, 9, 26, 9, 0) }).next.map(ymd),
    ['2026-11-02', '2026-11-09', '2026-11-16'], 'weekly from Monday');

// Times on next dates keep the start's time of day
const withTime = run('weekly', {});
eq(withTime.next[0].getHours() + ':' + withTime.next[0].getMinutes(), '10:0', 'next keeps start time');

// Errors
const ERR = [
    ['yearly', 'yearly is not supported yet'],
    ['annually', 'yearly is not supported yet'],
    ['weekly on thu', 'Tue'],                  // starts Tue, days must include it
    ['weekly on funday', 'funday'],
    ['monthly 2nd tue', 'starts on 27 Oct 2026'],    // 27 Oct is the 4th Tue, and also the last
    ['monthly 5th tue', 'fifth'],
    ['monthly 28', 'would not begin'],
    ['monthly 31st', 'would not begin'],
    ['monthly last', "didn't understand"],
    ['monthly 2nd xyz', 'xyz'],
    ['every 2 months', "didn't understand"],
    ['fortnightly', "didn't understand"],
    ['every 2 weeks on', "didn't understand"],
    ['daily until', "didn't understand"],
    ['daily until 2026-10-01', 'before it starts'],
    ['daily until 31 feb', 'no 31 Feb'],
    ['daily until 13 dec 2026 extra', 'until'],
    ['daily for 0', "'for 0'"],
    ['until 29 dec', "say what repeats"],     // range without a pattern
    ['never', "say what repeats"],
    ['for 10', "say what repeats"],
];
for (const row of ERR) {
    const r = run(row[0]);
    eq(r.ok, false, 'error for ' + JSON.stringify(row[0]));
    if (typeof r.error !== 'string' || r.error.indexOf(row[1]) < 0) {
        fail('error for ' + JSON.stringify(row[0]) + ' should contain ' + JSON.stringify(row[1]) + ', was ' + JSON.stringify(r));
    }
    checks++;
}
// Exact yearly message
eq(run('yearly').error, 'yearly is not supported yet', 'yearly exact message');

// weekdays from a weekend start is an error
const SAT = new Date(2026, 9, 31, 10, 0);
eq(run('weekdays', { start: SAT }).ok, false, 'weekdays from Saturday errors');
eq(run('weekly', { start: SAT }).pattern.daysOfWeek, ['saturday'], 'weekly from Saturday uses Saturday');

// Weekly-day mismatch error names the start day and suggests the fix
eq(run('weekly on thu').error.indexOf("Try 'weekly on tue,thu'") >= 0, true, 'mismatch suggests fix');

// listChoices: picker from the start date
function labels(list) { return list.map(function (c) { return c.label; }); }
function texts(list) { return list.map(function (c) { return c.text; }); }

eq(labels(R.listChoices(new Date(2026, 9, 27, 10, 0))),
    ['None', 'Daily', 'Weekdays', 'Weekly on Tue', 'Every 2 weeks on Tue', 'Monthly on the 27th',
        'Monthly on the 4th Tue', 'Monthly on the last Tue'], 'choices Tue 27 Oct');
eq(texts(R.listChoices(new Date(2026, 9, 27, 10, 0))),
    ['', 'daily', 'weekdays', 'weekly on tue', 'every 2 weeks on tue', 'monthly', 'monthly 4th tue', 'monthly last tue'],
    'choice texts Tue 27 Oct');

// Saturday 31 Oct: no Weekdays, 31st is the 5th weekday so no nth entry, last Sat offered
eq(labels(R.listChoices(new Date(2026, 9, 31, 10, 0))),
    ['None', 'Daily', 'Weekly on Sat', 'Every 2 weeks on Sat', 'Monthly on the 31st', 'Monthly on the last Sat'],
    'choices Sat 31 Oct');

// Mon 5 Oct: first Monday, not in the last 7 days
eq(labels(R.listChoices(new Date(2026, 9, 5, 10, 0))),
    ['None', 'Daily', 'Weekdays', 'Weekly on Mon', 'Every 2 weeks on Mon', 'Monthly on the 5th', 'Monthly on the 1st Mon'],
    'choices Mon 5 Oct');

// Thu 29 Oct: 5th Thursday, so no nth entry; last Thu is offered
eq(labels(R.listChoices(new Date(2026, 9, 29, 10, 0))),
    ['None', 'Daily', 'Weekdays', 'Weekly on Thu', 'Every 2 weeks on Thu', 'Monthly on the 29th', 'Monthly on the last Thu'],
    'choices Thu 29 Oct');

// Sat 24 Oct: 24 + 7 = 31 is still in the month, so no 'last' entry
eq(labels(R.listChoices(new Date(2026, 9, 24, 10, 0))).indexOf('Monthly on the last Sat'), -1, 'no last entry on 24th');
eq(labels(R.listChoices(new Date(2026, 9, 25, 10, 0)))[6], 'Monthly on the last Sun', 'last Sun offered from the 25th');

// Every non-empty choice text must parse back to ok from the same start date
const STARTS = [new Date(2026, 9, 27, 10, 0), new Date(2026, 9, 31, 10, 0), new Date(2026, 9, 5, 10, 0),
    new Date(2026, 9, 29, 10, 0), new Date(2026, 9, 25, 10, 0), new Date(2026, 9, 30, 10, 0)];
for (const s of STARTS) {
    for (const c of R.listChoices(s)) {
        const r = R.parseRepeat(c.text, { start: s, defaultSeriesMonths: 12 });
        if (c.text === '') { eq(r.pattern, null, 'None choice'); continue; }
        if (!r.ok) fail('choice ' + c.label + ' from ' + ymd(s) + ' does not parse: ' + r.error);
        checks++;
    }
}

console.log('recurrence tests ok (' + checks + ' checks, ' + OK.length + ' parse cases, ' + ERR.length + ' error cases)');
