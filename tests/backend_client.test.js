// R1.1 / ARCH-T1 guard: qml/state/Backend.qml is the only HTTP client for the backend.
// A request built anywhere else skips the port override (a scratch UI then writes to the live
// backend) and, from R1.4, the session token. Run from the project root: node tests/backend_client.test.js
const fs = require('fs');
const path = require('path');

const ROOT = 'qml';
const CLIENT = path.join('qml', 'state', 'Backend.qml');
const SKIP = new Set(['Commons', 'Ui']);           // symlinks to the installed Omarchy kit, not ours

function walk(dir, out) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    if (SKIP.has(e.name) && dir === ROOT) continue;
    const p = path.join(dir, e.name);
    if (e.isSymbolicLink()) continue;
    if (e.isDirectory()) walk(p, out);
    else if (/\.(qml|js|mjs)$/.test(e.name)) out.push(p);
  }
  return out;
}

const fail = [];
const files = walk(ROOT, []);
if (!files.includes(CLIENT)) fail.push(CLIENT + ' is missing');
for (const f of files) {
  if (f === CLIENT) continue;
  const lines = fs.readFileSync(f, 'utf8').split('\n');
  lines.forEach((l, i) => {
    if (/XMLHttpRequest/.test(l)) fail.push(`${f}:${i + 1}: XMLHttpRequest outside Backend.qml`);
    // A port literal means a hard-coded backend URL (the review found these in Tasks/People).
    if (/\b27182\b/.test(l)) fail.push(`${f}:${i + 1}: backend port literal outside Backend.qml`);
    if (/127\.0\.0\.1|localhost:\d/.test(l)) fail.push(`${f}:${i + 1}: loopback URL outside Backend.qml`);
  });
}
const qmldir = path.join('qml', 'state', 'qmldir');
if (!fs.existsSync(qmldir) || !/^singleton Backend 1\.0 Backend\.qml$/m.test(fs.readFileSync(qmldir, 'utf8')))
  fail.push(qmldir + ' must register `singleton Backend 1.0 Backend.qml`');

if (fail.length) { console.error('FAIL\n' + fail.join('\n')); process.exit(1); }
console.log(`ok: ${files.length} QML/JS files, only Backend.qml talks to the backend`);
