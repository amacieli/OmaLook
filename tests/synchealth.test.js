const fs = require('fs');
const path = require('path');
const src = fs.readFileSync(path.join(__dirname, '..', 'qml', 'common', 'synchealth.js'), 'utf8').replace('.pragma library', '');
const S = new Function(src + ';return {problems,warning,detail}')();
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
console.log('synchealth ok');
