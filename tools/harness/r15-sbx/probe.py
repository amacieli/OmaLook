import http.client, json, os, time, glob, subprocess
PORT=28155; H=os.environ["HOME"]
tokf=f"{os.environ['XDG_RUNTIME_DIR']}/omalook/ui-{PORT}.token"
for _ in range(100):
    if os.path.exists(tokf): break
    time.sleep(0.1)
TOK=open(tokf).read().strip()
def req(m,p,body=None,hdr=None,tok=True):
    c=http.client.HTTPConnection("127.0.0.1",PORT,timeout=15)
    h={"Host":f"127.0.0.1:{PORT}"}
    if tok: h["Authorization"]="Bearer "+TOK
    h.update(hdr or {})
    b=json.dumps(body) if body is not None else None
    c.request(m,p,body=b,headers=h); r=c.getresponse(); t=r.read().decode()
    return r.status,t
os.makedirs(f"{H}/.ssh",exist_ok=True)
sec=f"{H}/.ssh/id_ed25519"; open(sec,"w").write("PRIVATE KEY")
pick=f"{H}/report.txt"; open(pick,"w").write("hello")
jpg=f"{H}/face.jpg"; open(jpg,"wb").write(b"\xff\xd8\xff\xe0fake")
vcf=f"{H}/c.vcf"; open(vcf,"w").write("BEGIN:VCARD\nVERSION:3.0\nFN:X\nEND:VCARD\n")
req("GET","/accounts")  # warm up schema
def att(p,src=""): return {"path":p,"name":os.path.basename(p),"size":0,"source_path":src}
def msg(atts,draft=""): return {"account_id":"acct-x","to":[{"email":"a@example.com","name":""}],"subject":"s","body":{"format":"text","content":"x"},"attachments":atts,"draft_id":draft}
def spoolfiles(): return sorted(glob.glob(f"{H}/.cache/omalook/**/file",recursive=True))
R=[]
def rec(name,ok,detail): R.append(("PASS" if ok else "FAIL",name,detail[:160]))
s,t=req("POST","/files/register",{"purpose":"attach","paths":[pick]},tok=False); rec("register no token -> 401",s==401,f"{s}")
s,t=req("POST","/files/register",{"purpose":"attach","paths":[pick]},hdr={"Origin":"https://evil.example"}); rec("register with Origin -> 403",s==403,f"{s}")
s,t=req("POST","/files/register",{"purpose":"root","paths":[pick]}); rec("bad purpose refused","unknown purpose" in t,t)
s,t=req("POST","/compose/send",msg([att(sec)])); rec("send unregistered secret refused","not picked" in t and ".ssh" not in t,t)
before=spoolfiles()
s,t=req("POST","/drafts/save?state=saved",msg([att(sec)])); rec("draft unregistered secret refused, nothing spooled","not picked" in t and spoolfiles()==before,t)
s,t=req("POST","/files/register",{"purpose":"attach","paths":[pick, f"{H}/.ssh", f"{H}/nope"]}); j=json.loads(t)
rec("register: file ok, dir+missing refused",len(j["files"])==1 and len(j["refused"])==2,t)
canon=j["files"][0]["path"]
s,t=req("POST","/drafts/save?state=saved",msg([att(canon)])); j=json.loads(t)
rec("draft with registered file spools it",j.get("ok") is True and len(spoolfiles())==len(before)+1,t)
did=j.get("id",""); sp=j.get("attachments",[{}])[0].get("path","")
s,t=req("POST","/drafts/save?state=saved",msg([att(sp,sec)],did)); rec("draft: spool copy + swapped source refused","not picked" in t,t)
s,t=req("POST","/drafts/save?state=saved",msg([att(sp,canon)],did)); rec("draft: resave same spool+source ok",'"ok":true' in t,t)
lnk=os.path.join(os.path.dirname(sp),"evil"); os.symlink(sec,lnk)
s,t=req("POST","/compose/send",msg([att(lnk)])); rec("send: symlink in spool pointing out refused","not picked" in t,t)
s,t=req("POST","/compose/send",msg([att(sp,"/etc/passwd")])); rec("send: resumed-draft spool copy passes the file check","not picked" not in t,t)
s,t=req("POST","/compose/send",msg([att(f"{H}/.cache/omalook/drafts/../../.ssh/id_ed25519")])); rec("send: .. out of spool refused","not picked" in t,t)
s,t=req("POST","/contacts/photo/set",{"id":"c1","path":jpg}); rec("photo unregistered refused","pick the picture again" in t,t)
s,t=req("POST","/contacts/import",{"path":vcf,"account_id":"acct-x","folder_id":""}); rec("import unregistered refused","pick the file again" in t,t)
req("POST","/files/register",{"purpose":"attach","paths":[vcf]})
s,t=req("POST","/contacts/import",{"path":vcf,"account_id":"acct-x","folder_id":""}); rec("import: attach-registered file still refused","pick the file again" in t,t)
req("POST","/files/register",{"purpose":"photo","paths":[jpg]})
s,t=req("POST","/contacts/photo/set",{"id":"c1","path":jpg}); rec("photo registered passes the gate","pick the picture again" not in t,t)
for r in R: print(*r,sep=" | ")
print("SUMMARY", sum(r[0]=="PASS" for r in R), "/", len(R))
