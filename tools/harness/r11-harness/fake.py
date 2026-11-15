
import json, sys, threading
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler
log=open(sys.argv[1],'a',buffering=1)
LISTS={'/folders':[{"id":"f1","display_name":"Inbox","well_known_name":"inbox","unread_item_count":0,"total_item_count":1}],
 '/messages':[{"id":"m1","subject":"hello","from_name":"X","from_email":"x@example.com","received_date_time":"2026-11-12T10:00:00Z","is_read":True,"body_preview":"hi","folder_id":"f1","account_id":"exchange-x"}],
 '/accounts':[{"id":"exchange-x","provider":"exchange","email":"x@example.com","signed_in":True}], '/tasks':[{"id":"t1","title":"T","completed":False}], '/calendar/events':[], '/contacts':[]}
def mk(port):
  class H(BaseHTTPRequestHandler):
    def _do(self):
      n=int(self.headers.get('Content-Length') or 0); body=self.rfile.read(n) if n else b''
      path=self.path.split('?')[0]
      log.write(json.dumps({"port":port,"m":self.command,"path":self.path[:120],"origin":self.headers.get('Origin'),"host":self.headers.get('Host'),"ctype":self.headers.get('Content-Type'),"blen":len(body),"hdrs":sorted(k.lower() for k in self.headers.keys())})+"\n")
      if path=='/messages/body': out={"type":"html","content":"<p>hello</p>"}
      elif path=='/render/get': out={"hit":False}
      elif path=='/sync/serial': out={"mail":0,"sms":0}
      elif path in LISTS: out=LISTS[path]
      else: out={"ok":True}
      b=json.dumps(out).encode()
      self.send_response(200); self.send_header('Content-Type','application/json'); self.send_header('Content-Length',str(len(b))); self.end_headers(); self.wfile.write(b)
    do_GET=do_POST=do_PATCH=do_DELETE=do_PUT=_do
    def log_message(self,*a): pass
  return H
for port in (27182,27999):
  s=ThreadingHTTPServer(('127.0.0.1',port),mk(port)); threading.Thread(target=s.serve_forever,daemon=True).start()
print("fake up",flush=True)
threading.Event().wait()
