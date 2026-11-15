# R3.2 probe: requests never name files; handles, pickers, secret roots. Run with PROBE=probe_r32.py sbx.sh
import http.client, json, os, time
PORT=28155
tokf=f"{os.environ['XDG_RUNTIME_DIR']}/omalook/ui-{PORT}.token"
for _ in range(150):
    if os.path.exists(tokf): break
    time.sleep(0.1)
TOK=open(tokf).read().strip()
def req(m,p,body=None,tok=True,hdr=None):
    c=http.client.HTTPConnection("127.0.0.1",PORT,timeout=20)
    h={"Host":f"127.0.0.1:{PORT}"}
    if tok: h["Authorization"]="Bearer "+TOK
    h.update(hdr or {})
    b=json.dumps(body) if body is not None else None
    c.request(m,p,body=b,headers=h); r=c.getresponse(); t=r.read().decode(errors="replace")
    try: return r.status, json.loads(t)
    except Exception: return r.status, t
H=os.environ["HOME"]
os.makedirs(H+"/docs",exist_ok=True); open(H+"/docs/report.txt","w").write("hello")
os.makedirs(H+"/.ssh",exist_ok=True); open(H+"/.ssh/id_ed25519","w").write("SECRET")
req("GET","/accounts")
res=[]
def check(name, ok, detail=""):
    res.append(ok); print(("PASS " if ok else "FAIL ")+name+("" if ok else f"  -> {detail}"))
s,r=req("POST","/files/register",{"purpose":"attach","paths":[H+"/docs/report.txt", H+"/.ssh/id_ed25519", "/etc/hostname"]})
files=r.get("files",[]) if isinstance(r,dict) else []
check("register returns handles, no path", len(files)>=1 and files[0].get("handle","").startswith("fh-") and "path" not in files[0], r)
check("register refuses ~/.ssh", any(".ssh" in x.get("path","") for x in r.get("refused",[])), r)
h=files[0]["handle"]
s,r2=req("POST","/files/register",{"purpose":"attach","paths":[H+"/docs/report.txt"]})
check("same file -> same handle", r2["files"][0]["handle"]==h, r2)
msg=lambda atts: {"kind":"new","account_id":"exchange-x","to":[{"email":"a@example.com"}],"subject":"s","body":{"format":"text","content":"hi"},"attachments":atts}
s,r=req("POST","/compose/send",msg([{"path":"/etc/hostname","name":"hostname"}]))
check("compose/send with a path refused", isinstance(r,dict) and r.get("ok") is False and "picked" in r.get("error",""), r)
s,r=req("POST","/compose/send",msg([{"handle":"fh-"+"0"*32,"name":"x"}]))
check("compose/send with forged handle refused", isinstance(r,dict) and r.get("ok") is False, r)
s,r=req("POST","/drafts/save",msg([{"handle":h,"name":"report.txt","size":5}]))
check("drafts/save with a handle accepted", isinstance(r,dict) and r.get("ok") is True and all("path" not in a and a.get("handle","").startswith("fh-") for a in r.get("attachments",[])), r)
if isinstance(r,dict) and r.get("id"):
    s,g=req("GET","/drafts/get?id="+r["id"])
    atts=(g.get("draft") or {}).get("payload",{}).get("attachments",[]) if isinstance(g,dict) else []
    check("drafts/get returns handles + localPath, no path", len(atts)==1 and atts[0].get("handle","").startswith("fh-") and "path" not in atts[0] and atts[0].get("localPath"), g)
s,r=req("GET","/fs/list?path="+H)
check("fs/list without picker refused", isinstance(r,dict) and r.get("error")=="picker expired", r)
s,p=req("POST","/files/picker",{"purpose":"attach"})
pid=p.get("picker","")
s,r=req("GET",f"/fs/list?picker={pid}&path={H}/docs")
check("fs/list with picker lists", isinstance(r,dict) and any(e.get("name")=="report.txt" for e in r.get("entries",[])), r)
for d in [H+"/.ssh", H+"/.config/omalook", "/proc/self"]:
    s,r=req("GET",f"/fs/list?picker={pid}&path={d}")
    check("fs/list refuses "+d, isinstance(r,dict) and "entries" not in r, str(r)[:200])
s,r=req("POST","/contacts/photo/set",{"id":"c1","path":"/etc/hostname"})
check("contacts/photo/set with a path refused", isinstance(r,dict) and r.get("ok") is not True, r)
for m,pth,b in [("POST","/files/picker",{"purpose":"attach"}),("GET","/fs/list?picker="+pid+"&path="+H,None)]:
    s1,_=req(m,pth,b,tok=False); s2,_=req(m,pth,b,hdr={"Origin":"null"})
    check(f"guard {m} {pth.split('?')[0]}", s1==401 and s2==403, (s1,s2))
print(f"{sum(res)}/{len(res)} passed")
