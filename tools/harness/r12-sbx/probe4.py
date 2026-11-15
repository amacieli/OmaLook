
import json, sys, time, http.client, os, stat, subprocess
out=sys.argv[1]; label=sys.argv[2]; BIN=sys.argv[3]
TF="/run/user/1000/omalook/ui-27182.token"
def req(path, method="GET", body=None, headers=None, host="127.0.0.1:27182", tok=None):
    c=http.client.HTTPConnection("127.0.0.1",27182,timeout=10)
    try:
        c.putrequest(method,path,skip_host=True,skip_accept_encoding=True)
        c.putheader("Host", host)
        if tok: c.putheader("Authorization","Bearer "+tok)
        for k,v in (headers or {}).items(): c.putheader(k,v)
        if body is not None: c.putheader("Content-Length",str(len(body)))
        c.endheaders(body.encode() if isinstance(body,str) else body)
        r=c.getresponse(); d=r.read().decode("utf8","replace")
        return [r.status, d[:120]]
    except Exception as e: return [-1,str(e)[:100]]
    finally: c.close()
for i in range(150):
    if os.path.exists(TF): break
    time.sleep(0.2)
T=open(TF).read().strip() if os.path.exists(TF) else ""
for i in range(150):
    if req("/sync/serial",tok=T)[0]==200: break
    time.sleep(0.2)
req("/accounts",tok=T)
r={}
st=os.stat(TF); sd=os.stat(os.path.dirname(TF))
r["token_file"]={"mode":oct(st.st_mode&0o777),"dir_mode":oct(sd.st_mode&0o777),"len":len(T)}
r["with_token"]=req("/accounts",tok=T)
r["no_token"]=req("/accounts")
r["wrong_token"]=req("/accounts",tok="f"*64)
r["origin_with_token"]=req("/accounts",tok=T,headers={"Origin":"https://evil.example"})
r["serial_no_token"]=req("/sync/serial")
gets=["/sync/serial","/folders","/accounts","/messages?limit=5&offset=0","/messages/body?id=x","/settings/ui","/settings/mail","/settings/mcp","/settings/senders","/settings/signatures","/fs/list?path=/etc","/contacts?view=all&sort=first","/contacts/photo?id=x","/drafts","/rules","/local-rules","/tags","/calendar/events?month=2026-11","/calendar/find?account=x","/compose/originals?id=x","/tasks","/sms/threads","/render/get?id=x","/nonexistent"]
posts=["/settings/mcp/enabled?value=true","/settings/mcp/domain?domain=mail&level=write","/settings/mcp/domain?domain=calendar&level=write","/settings/mcp/domain?domain=contacts&level=read","/settings/mcp/domain?domain=sms&level=read","/settings/mcp/tool?name=calendar_create_event&mode=allow","/settings/background?enabled=true",
       "/settings/mcp/token/regenerate","/settings/mcp/copy?what=token","/compose/send","/drafts/save?state=saved","/settings/signature?account=a","/settings/senders/add?email=a@evil.example","/accounts/remove?account=exchange-x","/auth/login?provider=gmail","/auth/logout?account=x","/messages/action","/perf?m=x","/settings/view_state","/tasks/t1/complete","/contacts/quick","/rules/add?account=a&tag=t&address=b@c.d"]
r["routes_no_token"]={}
for p in gets: r["routes_no_token"]["GET "+p]=req(p)[0]
for p in posts: r["routes_no_token"]["POST "+p]=req(p,"POST",'{"text":"x"}',{"Content-Type":"text/plain"})[0]
r["all_401"]=all(v==401 for v in r["routes_no_token"].values())
r["mcp_after"]=req("/settings/mcp",tok=T)[1][:200]
r["background_after"]=req("/settings/background",tok=T)
r["senders_after"]=req("/settings/senders",tok=T)
r["signatures_after"]=req("/settings/signatures",tok=T)
log=open("/mnt/d/r12-sbx/out/"+label+".backend.log").read()
r["token_in_log"]=T in log
r["stubs_log"]=open("/mnt/d/r12-sbx/out/stubs.log").read() if os.path.exists("/mnt/d/r12-sbx/out/stubs.log") else ""
json.dump(r,open(out,"w"),indent=1)
