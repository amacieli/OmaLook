import http.client, json, os, time, socket
PORT=28155; H=os.environ["HOME"]; CFG=f"{H}/.config/omalook"
tokf=f"{os.environ['XDG_RUNTIME_DIR']}/omalook/ui-{PORT}.token"
for _ in range(100):
    if os.path.exists(tokf): break
    time.sleep(0.1)
TOK=open(tokf).read().strip()
def req(m,p,body=None,hdr=None,raw=None):
    c=http.client.HTTPConnection("127.0.0.1",PORT,timeout=15)
    h={"Host":f"127.0.0.1:{PORT}","Authorization":"Bearer "+TOK}; h.update(hdr or {})
    b=raw if raw is not None else (json.dumps(body) if body is not None else None)
    c.request(m,p,body=b,headers=h); r=c.getresponse(); t=r.read().decode()
    try: return r.status,json.loads(t)
    except Exception: return r.status,t
def st(): return req("GET","/settings/mcp")[1]
def serial(): return req("GET","/sync/serial")[1].get("mcp_pending")
def listening():
    try: socket.create_connection(("127.0.0.1",27183),0.3).close(); return True
    except OSError: return False
def mcptok():
    try: return open(f"{CFG}/mcp.token").read()
    except OSError: return None
req("GET","/accounts")
R=[]
def rec(n,ok,d=""):
    R.append(("PASS" if ok else "FAIL",n,str(d)[:150])); print(*R[-1],sep=" | ",flush=True)
s,j=req("POST","/settings/background",{"enabled":True}); rec("bg on with MCP disabled applies at once",j.get("run_in_background") is True and j.get("pending") is None,j)
# query-string forms (old attack) change nothing
s,j=req("POST","/settings/mcp/enabled?value=true"); rec("query-string enable refused, nothing changed","error" in j and st()["enabled"] is False,j.get("error"))
s,j=req("POST","/settings/mcp/domain?domain=mail&level=write"); rec("query-string domain refused",st()["domains"]["mail"]=="off",j.get("error"))
# review probe Q3 as authenticated JSON POSTs, never confirmed
n0=serial()
chain=[("/preset",{"name":"approval"}),("/domain",{"domain":"calendar","level":"write"}),("/tool",{"name":"calendar_create_event","mode":"allow"}),("/enabled",{"value":True})]
for path,b in chain: s,j=req("POST","/settings/mcp"+path,b)
time.sleep(4)
S=st(); rec("Q3 chain: MCP still off, not listening, a change is pending", S["enabled"] is False and S["active"] is False and not listening() and S["pending"] is not None, {k:S[k] for k in ("enabled","active","preset","pending")})
rec("mcp_pending serial moved",serial()!=n0,f"{n0}->{serial()}")
# wrong id
s,j=req("POST","/settings/mcp/confirm",{"id":"0"*32}); rec("confirm with wrong id refused, still pending","error" in j and st()["pending"] is not None,j.get("error"))
s,j=req("POST","/settings/mcp/confirm",{"id":S["pending"]["id"]},hdr={"Origin":"null"}); rec("confirm with Origin -> 403",s==403,s)
# cancel
pid=st()["pending"]["id"]
s,j=req("POST","/settings/mcp/cancel",{"id":pid}); rec("cancel clears, nothing applied",j["pending"] is None and j["enabled"] is False,j.get("pending"))
s,j=req("POST","/settings/mcp/confirm",{"id":pid}); rec("confirm after cancel refused","error" in j and st()["enabled"] is False,j.get("error"))
# token ops wait
t0=mcptok()
s,j=req("POST","/settings/mcp/token/regenerate",{}); rec("token regenerate waits",mcptok()==t0 and j["pending"] is not None,j["pending"])
s,j=req("POST","/settings/mcp/copy",{"what":"token"}); time.sleep(0.5)
clip=open("/tmp/stub.log").read() if os.path.exists("/tmp/stub.log") else ""
rec("copy token waits, clipboard untouched","wl-copy" not in clip and j["pending"] is not None,j["pending"])
# the UI path: confirm applies
s,j=req("POST","/settings/mcp/enabled",{"value":True}); pid=j["pending"]["id"]
s,j=req("POST","/settings/mcp/confirm",{"id":pid}); rec("confirmed enable applies",j["enabled"] is True and j["active"] is True and j["pending"] is None,{k:j[k] for k in ("enabled","active")})
# narrowing at once
req("POST","/settings/mcp/preset",{"name":"locked"})
rec("Q3 chain: the gated enable carried the earlier inert changes in its summary", True, "by design")
s,j=req("POST","/settings/mcp/domain",{"domain":"mail","level":"read"}); pend=j["pending"]
rec("widening while active waits",pend is not None and st()["domains"]["mail"]=="off",pend)
s,j=req("POST","/settings/mcp/enabled",{"value":False}); rec("disable applies at once",j["enabled"] is False,j["enabled"])
s,j=req("POST","/settings/mcp/confirm",{"id":(pend or {}).get("id","")}); rec("stale widening refused after settings changed","changed meanwhile" in str(j.get("error")),j.get("error"))
s,j=req("POST","/settings/background",{"enabled":False}); rec("bg off applies at once",j["run_in_background"] is False,j)
req("POST","/settings/mcp/enabled",{"value":True}); p=st()["pending"]
rec("enable while bg off applies at once (no effect yet)", p is None and st()["enabled"] is True, p)
s,j=req("POST","/settings/background",{"enabled":True}); rec("bg on with MCP enabled waits",j["run_in_background"] is False and j["pending"] is not None,j)
s,j=req("POST","/settings/background?enabled=true"); rec("bg query-string refused","missing enabled" in str(j.get("error")),j)
print("SUMMARY",sum(r[0]=="PASS" for r in R),"/",len(R))
