# Replays requests from $REQS (JSON list of [method, path, body-or-null]) against the sandboxed backend
# and writes one JSON line per request to $OUT: {m, p, status, body}. Also checks every request is
# refused without the token (401) and with an Origin (403).
import http.client, json, os, time
PORT=28155
tokf=f"{os.environ['XDG_RUNTIME_DIR']}/omalook/ui-{PORT}.token"
for _ in range(150):
    if os.path.exists(tokf): break
    time.sleep(0.1)
TOK=open(tokf).read().strip()
def req(m,p,body=None,hdr=None,tok=True):
    c=http.client.HTTPConnection("127.0.0.1",PORT,timeout=20)
    h={"Host":f"127.0.0.1:{PORT}"}
    if tok: h["Authorization"]="Bearer "+TOK
    h.update(hdr or {})
    b=(body if isinstance(body,str) else json.dumps(body)) if body is not None else None
    c.request(m,p,body=b,headers=h); r=c.getresponse(); t=r.read().decode(errors="replace")
    return r.status,t
req("GET","/accounts")  # warm up schema
if os.environ.get("SEED"):
    import sqlite3
    _db = sqlite3.connect(os.environ["XDG_CONFIG_HOME"] + "/omalook/messages.db", timeout=20)
    _db.executescript(open(os.environ["SEED"]).read())
    _db.close()
    print("seeded")
reqs=json.load(open(os.environ.get("REQS","/mnt/d/r3-sbx/reqs.json")))
out=open(os.environ.get("OUT","/mnt/d/r3-sbx/out.jsonl"),"w")
bad=0
for m,p,b in reqs:
    s1,_=req(m,p,b,tok=False); s2,_=req(m,p,b,hdr={"Origin":"null"})
    if s1!=401 or s2!=403: bad+=1; print(f"GUARD FAIL {m} {p}: no-token {s1}, origin {s2}")
    s,t=req(m,p,b)
    out.write(json.dumps({"m":m,"p":p,"status":s,"body":t[:500000]})+"\n")
print(f"{len(reqs)} requests replayed, guard failures: {bad}")
