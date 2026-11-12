// "This and following" (plan 5.7): the draft.js lines the editor, the dialog and the pending marks use.
const fs = require('fs');
const src = fs.readFileSync(__dirname + '/../qml/calendar/draft.js', 'utf8').replace(/^\.pragma.*$/m, '');
const D = new Function(src + ';return {reachLine,changePath,changeMessage,removeLine,followingLine,dayBefore,dayOf,applyChanges}')();
let n = 0;
function eq(a, b, m) { n++; if (JSON.stringify(a) !== JSON.stringify(b)) { console.error('FAIL', m, '\n got', JSON.stringify(a), '\nwant', JSON.stringify(b)); process.exit(1); } }

eq(D.dayBefore('2026-11-03'), '2026-11-02', 'day before');
eq(D.dayBefore('2026-03-01'), '2026-02-28', 'day before across a month');
eq(D.dayOf({ start: '2026-11-03T14:00:00' }), '2026-11-03', 'day of a cached event');

eq(D.changePath('update', 'x', 'following', '', false), '/calendar/event?id=x&scope=following', 'following edit path');
eq(D.changePath('delete', 'x', 'following', 'delete', true), '/calendar/event/cancel?id=x&scope=following&kind=delete&dry_run=1', 'following delete dry-run path');
eq(D.changePath('update', 'x', 'bogus', '', false), '/calendar/event?id=x&scope=occurrence', 'unknown scope still falls to occurrence');

eq(D.followingLine('update', 3, '2026-11-03'), 'The series ends 2026-11-02. A new series from 2026-11-03 is sent to 3 people when the undo window ends.', 'following edit, people');
eq(D.followingLine('update', 0, '2026-11-03'), 'The series ends 2026-11-02. A new series from 2026-11-03 is personal.', 'following edit, personal');
eq(D.followingLine('update', 1, '2026-11-03'), 'The series ends 2026-11-02. A new series from 2026-11-03 is sent to 1 person when the undo window ends.', 'following edit, one person');
eq(D.followingLine('update', -1, '2026-11-03'), 'Counting who is invited\u2026', 'following counting');
eq(D.followingLine('delete', 0, '2026-11-03'), 'Removes this and the following occurrences from your calendar when the undo window ends. Nobody is told.', 'following delete');
eq(D.removeLine('delete', 0, true, '2026-11-03'), D.followingLine('delete', 0, '2026-11-03'), 'removeLine routes following');
eq(D.removeLine('delete', 0, false, ''), 'Removed from your calendar when the undo window ends. Nobody is told.', 'plain delete unchanged');

eq(D.reachLine(2, 'edit', 'following', '2026-11-03'), 'The series ends 2026-11-02. A new series from 2026-11-03 is sent to 2 people when the undo window ends.', 'editor footer for following');
eq(D.reachLine(2, 'edit', 'series', ''), 'Invitations with the update go out to 2 people when the undo window ends.', 'editor footer for series unchanged');

eq(D.changeMessage('update', 2, true, true), 'Change queued on this and following occurrences for 2 people', 'undo bar, following');
eq(D.changeMessage('update', 2, true, false), 'Change queued on the whole series for 2 people', 'undo bar, series unchanged');

// Pending marks: a following edit marks the picked occurrence and the ones after it, and changes no times.
const rows = [
  { id: 'o1', series_master_id: 'M', subject: 'Standup', start: '2026-11-02T09:00:00', end: '2026-11-02T09:15:00', event_type: 'occurrence' },
  { id: 'o2', series_master_id: 'M', subject: 'Standup', start: '2026-11-03T09:00:00', end: '2026-11-03T09:15:00', event_type: 'occurrence' },
  { id: 'o3', series_master_id: 'M', subject: 'Standup', start: '2026-11-10T09:00:00', end: '2026-11-10T09:15:00', event_type: 'occurrence' },
];
const items = [{ op: 'update', state: 'pending', target: 'M', series: true, following: true, occurrence: '2026-11-03',
  subject: 'Renamed', start: '2026-11-03T10:00:00', end: '2026-11-03T10:15:00', all_day: false }];
const out = D.applyChanges(rows, items);
eq(out[0].subject, 'Standup', 'before the picked occurrence: untouched');
eq(out[0].start, '2026-11-02T09:00:00', 'before: time untouched');
eq(out[1].subject, '\u23f2 Standup', 'picked occurrence marked');
eq(out[1].start, '2026-11-03T09:00:00', 'picked occurrence: no new time before the server has it');
eq(out[2].subject, '\u23f2 Standup', 'later occurrence marked');
eq(out[2].pending, true, 'later occurrence pending');
// A series edit still changes the times of every occurrence (unchanged behaviour).
const plain = D.applyChanges(rows, [{ op: 'update', state: 'pending', target: 'M', series: true, subject: 'Renamed',
  start: '2026-11-03T10:00:00', end: '2026-11-03T10:15:00', all_day: false }]);
eq(plain[0].start, '2026-11-03T10:00:00', 'series edit still moves times');
console.log('calendar_following.test.js: ' + n + ' checks passed');
