#!/usr/bin/env python3
"""Round trip for remote <img> tags in DocumentHandler (OL-054 / OL-048 resume data loss).

Runs a headless QML scene (QT_QPA_PLATFORM=offscreen) that loads an HTML body with remote
pictures, inserts a cid: picture, edits, and saves/resumes twice. Asserts:
  - every remote <img> tag comes back from html() byte-for-byte (http, https, protocol-relative);
  - the cid: picture survives;
  - a second and third resume give the same HTML;
  - a dummy HTTP server on 127.0.0.1 sees ZERO requests from the editor;
  - control: a plain TextEdit with the same remote <img> DOES request it (proves the server works).

Usage: python3 remote_images_roundtrip.py <dir containing OmaLook/Compose/qmldir>
  (default: ../build/qml next to this file). Needs /usr/lib/qt6/bin/qml.
"""
import http.server, os, struct, subprocess, sys, tempfile, threading, time, zlib

PORT = 28191
REMOTE = ["<img src=\"http://127.0.0.1:%d/pic.png\" alt=\"logo\" width=\"80\" height=\"30\">" % PORT,
          "<img src=\"https://127.0.0.1:%d/x.png\">" % PORT,
          "<img src=\"//127.0.0.1:%d/proto.png\" alt=\"p\">" % PORT]

MAIN = r'''
import QtQuick
import OmaLook.Compose
Item {
    width: 600; height: 400
    TextEdit { id: ed; width: 600; height: 400; textFormat: TextEdit.RichText; wrapMode: TextEdit.Wrap }
    DocumentHandler { id: h; document: ed.textDocument }
    Timer {
        id: step; interval: 800; property int n: 0
        onTriggered: {
            n++
            if (n === 1) {
                h.setHtml('<p>Hello <b>draft</b></p>' + BODY_IMGS + '<p>tail</p>')
                h.insertImage("PIC", "c1")
                ed.cursorPosition = ed.length
                ed.insert(ed.length, " typed")
                console.log("PHASE1 " + h.html().replace(/\n/g, " "))
                step.restart()
            } else if (n === 2) {
                var a = h.html()
                h.setHtml(a); var b = h.html()
                h.setHtml(b); var c = h.html()
                console.log("PHASE2 " + c.replace(/\n/g, " "))
                console.log("STABLE " + (a === b && b === c))
                step.restart()
            } else { console.log("DONE"); Qt.quit() }
        }
    }
    Component.onCompleted: step.restart()
}
'''

CONTROL = r'''
import QtQuick
Item {
    width: 600; height: 400
    TextEdit { width: 600; height: 400; textFormat: TextEdit.RichText; text: '<p>x</p>CONTROL_IMG' }
    Timer { interval: 2000; running: true; onTriggered: { console.log("DONE"); Qt.quit() } }
}
'''

def png(w, h):
    raw = b''.join(b'\x00' + b'\xff\x00\x00' * w for _ in range(h))
    def chunk(t, d): return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b'')

class Dummy(http.server.BaseHTTPRequestHandler):
    hits = []
    def do_GET(self):
        Dummy.hits.append(self.path); self.send_response(404); self.end_headers()
    def log_message(self, *a): pass

def run_qml(qml_dir, src, tmp, name, subs):
    for k, v in subs.items(): src = src.replace(k, v)
    path = os.path.join(tmp, name + '.qml')
    open(path, 'w').write(src)
    env = dict(os.environ, QT_QPA_PLATFORM='offscreen', QML_IMPORT_PATH=qml_dir, QT_FORCE_STDERR_LOGGING='1')
    r = subprocess.run(['/usr/lib/qt6/bin/qml', path], env=env, capture_output=True, text=True, timeout=90)
    return r.stdout + r.stderr

def main():
    here = os.path.dirname(os.path.abspath(__file__))
    qml_dir = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else os.path.join(here, '..', 'build', 'qml'))
    tmp = tempfile.mkdtemp(prefix='remote-img-rt-')
    pic = os.path.join(tmp, 'pic.png'); open(pic, 'wb').write(png(4, 4))
    srv = http.server.ThreadingHTTPServer(('127.0.0.1', PORT), Dummy)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    failures = []
    try:
        out = run_qml(qml_dir, MAIN, tmp, 'main', {'BODY_IMGS': ' + '.join("'%s'" % t.replace("'", "\\'") for t in REMOTE).replace(" + ", " + ") , 'PIC': pic})
        out = out.replace('\r', '')
        p1 = [l.split('PHASE1 ', 1)[1] for l in out.splitlines() if 'PHASE1 ' in l]
        p2 = [l.split('PHASE2 ', 1)[1] for l in out.splitlines() if 'PHASE2 ' in l]
        if not p1 or not p2: failures.append('no PHASE output:\n' + out[-2000:])
        else:
            for t in REMOTE:
                if t not in p1[0]: failures.append('after set/edit, tag missing: ' + t)
                if t not in p2[0]: failures.append('after resume x2, tag missing: ' + t)
            if 'src="cid:c1"' not in p2[0]: failures.append('cid: picture lost')
        if 'STABLE true' not in out.replace('qml: ', ''): failures.append('resume is not stable (html changes on each save)')
        if Dummy.hits: failures.append('editor requested remote URLs: %s' % Dummy.hits)
        print('editor run:', 'ok' if not failures else 'FAILED')
        Dummy.hits.clear()
        ctl = run_qml(qml_dir, CONTROL, tmp, 'control', {'CONTROL_IMG': "<img src=\"http://127.0.0.1:%d/control.png\" width=\"20\" height=\"20\">" % PORT})
        if not Dummy.hits: failures.append('control: plain TextEdit did not request the remote image; harness is blind')
        print('control requests seen:', Dummy.hits)
    finally:
        time.sleep(0.3); srv.shutdown()
    print('FAIL' if failures else 'PASS')
    for f in failures: print(' -', f)
    sys.exit(1 if failures else 0)

if __name__ == '__main__':
    main()
