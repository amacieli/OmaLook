const fs = require('fs');
const path = require('path');
const src = fs.readFileSync(path.join(__dirname, '..', 'qml', 'common', 'synchealth.js'), 'utf8').replace('.pragma library', '');
const S = new Function(src + ';return {problems,warning,detail,dbWarning,dbDetail}')();
const assert = require('assert');

const accts = [{ id: 'a1', email: 'me@work.com' }, { id: 'a2', email: 'x@y.org', display_name: 'Home' }];
const T = (name, state, account, extra) => Object.assign({ name, account: account || null, state, restarts: 0 }, extra);

assert.strictEqual(S.warning([], accts), '');
assert.strictEqual(S.warning(null, accts), '');
assert.strictEqual(S.warning([T('mail', 'running', 'a1')], accts), '');
assert.strictEqual(S.warning([T('mail', 'stopped', 'a1')], accts), 'Sync stopped: mail (me@work.com)');
assert.strictEqual(S.warning([T('mail', 'stopped', 'a2')], accts), 'Sync stopped: mail (Home)');
assert.strictEqual(S.warning([T('calendar', 'restarting')], accts), 'Sync restarting: calendar');
assert.strictEqual(S.warning([T('mail', 'stopped', 'zz')], accts), 'Sync stopped: mail');
// stopped wins over restarting
assert.strictEqual(S.warning([T('calendar', 'restarting'), T('mail', 'stopped', 'a1')], accts), 'Sync stopped: mail (me@work.com)');
assert.strictEqual(S.warning([T('a', 'stopped'), T('b', 'stopped'), T('c', 'restarting')], accts), 'Sync stopped: a +1 more');
assert.strictEqual(S.problems([T('a', 'running'), T('b', 'restarting')]).length, 1);
assert.strictEqual(S.detail([T('mail', 'restarting', 'a1', { restarts: 2, last_panic: 'boom' })], accts),
  'mail (me@work.com) \u2014 restarting, 2 restarts: boom');
assert.strictEqual(S.detail([], accts), '');
// database health
assert.strictEqual(S.dbWarning(null), '');
assert.strictEqual(S.dbWarning(undefined), '');
assert.strictEqual(S.dbWarning({}), '');
assert.strictEqual(S.dbWarning({ state: 'ok' }), '');
assert.strictEqual(S.dbWarning({ state: 'unchecked' }), '');
assert.strictEqual(S.dbWarning({ state: 'damaged', detail: 'x' }), 'Database damaged: Settings > General > Rebuild');
assert.strictEqual(S.dbWarning({ state: 'rebuild_requested' }), 'Database rebuild at next start');
assert.strictEqual(S.dbWarning({ state: 'rebuilt' }), 'Database rebuilt: mail is downloading again');
assert.strictEqual(S.dbWarning({ state: 'rebuilt', lost: [] }), 'Database rebuilt: mail is downloading again');
assert.strictEqual(S.dbWarning({ state: 'rebuilt', lost: ['messages', 'folders'] }),
  'Database rebuilt: mail is downloading again (lost: messages, folders)');
assert.strictEqual(S.dbWarning({ state: 'bogus' }), '');
assert.strictEqual(S.dbDetail(null), '');
assert.strictEqual(S.dbDetail({}), '');
assert.strictEqual(S.dbDetail({ state: 'ok' }), '');
assert.strictEqual(S.dbDetail({ state: 'unchecked', detail: 'x' }), '');
assert.strictEqual(S.dbDetail({ state: 'damaged' }), 'Database damaged');
assert.strictEqual(S.dbDetail({ state: 'damaged', detail: 'page 7 bad' }), 'Database damaged\npage 7 bad');
assert.strictEqual(S.dbDetail({ state: 'rebuild_requested' }), 'Database rebuild requested: happens at the next start');
assert.strictEqual(S.dbDetail({ state: 'rebuilt', lost: ['messages'] }), 'Database rebuilt at this start\nLost: messages');
assert.strictEqual(S.dbDetail({ state: 'rebuilt', detail: 'ok', lost: [] }), 'Database rebuilt at this start\nok');
console.log('synchealth ok');
