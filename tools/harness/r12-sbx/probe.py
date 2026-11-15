
import socket, time, json, sys, threading, http.client
PORT=27182
out=sys.argv[1]; label=sys.argv[2]
res={}
def get(path, method="GET", body=None, headers=None, timeout=15):
    t=time.time()
    c=http.client.HTTPConnection("127.0.0.1",PORT,timeout=timeout)
    try:
        c.request(method,path,body=body,headers=headers or {})
        r=c.getresponse(); data=r.read()
        return r.status, data, (time.time()-t)*1000
    except Exception as e:
        return -1, str(e).encode(), (time.time()-t)*1000
    finally: c.close()
# wait for backend
for i in range(150):
    s,_,_=get("/sync/serial",timeout=2)
    if s==200: break
    time.sleep(0.2)
res["up"]=s
get("/accounts",timeout=20)  # warm schema
# --- route snapshot (GET only, read-only)
routes=["/sync/serial","/folders","/accounts","/messages?limit=5&offset=0","/settings/ui","/settings/mail","/settings/render","/settings/calendar",
        "/settings/keys","/settings/signatures","/settings/background","/settings/mcp","/settings/meetings","/settings/senders","/settings/view_state",
        "/search/history","/lists","/lists/targets","/tags","/local-rules","/drafts","/calendar/outbox","/calendar/events?month=2026-11","/contacts?view=all&sort=first",
        "/contacts/folders","/tasks","/categories","/compose/identities","/attach/limits?account=x","/messages/meta_progress","/addresses","/nonexistent"]
snap={}
for r in routes:
    s,d,ms=get(r)
    snap[r]=[s,d.decode("utf8","replace")]
res["snapshot"]=snap
# --- P1 idle socket held open, then requests must answer fast
idle=socket.create_connection(("127.0.0.1",PORT))
time.sleep(0.3)
p1=[]
for r in ["/sync/serial","/accounts","/settings/ui"]:
    s,_,ms=get(r,timeout=10); p1.append([r,s,round(ms,1)])
res["P1_idle_socket"]=p1
# --- P2 lying Content-Length held open; others answer; liar gets 408 or close
liar=socket.create_connection(("127.0.0.1",PORT))
liar.sendall(b"POST /settings/mail HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 100\r\n\r\nshort")
t0=time.time()
p2=[]
for r in ["/sync/serial","/folders"]:
    s,_,ms=get(r,timeout=10); p2.append([r,s,round(ms,1)])
liar.settimeout(15)
try: ans=liar.recv(200).decode(errors="replace").split("\r\n")[0]
except Exception as e: ans="exc "+str(e)
res["P2_lying_length"]={"others":p2,"liar_answer":ans,"liar_after_ms":round((time.time()-t0)*1000)}
idle.close(); liar.close()
# --- P3 1500-byte signature
text="S"*1480+" end-marker"
s,d,ms=get("/settings/signature?account=exchange-probe","POST",json.dumps({"text":text}).encode(),{"Content-Type":"application/json"})
s2,d2,_=get("/settings/signatures")
try: saved=json.loads(d2).get("exchange-probe","")
except Exception: saved=d2.decode()[:200]
res["P3_signature"]={"post":[s,d.decode()[:200]],"saved_len":len(saved) if isinstance(saved,str) else saved,"equal":saved==text}
# --- P4 connection cap: 40 idle sockets
socks=[]
for i in range(40):
    try: socks.append(socket.create_connection(("127.0.0.1",PORT),timeout=3))
    except Exception as e: break
time.sleep(0.5)
refused=0
for so in socks:
    so.settimeout(0.3)
    try:
        b=so.recv(100)
        if b.startswith(b"HTTP/1.1 503"): refused+=1
    except Exception: pass
s,_,ms=get("/sync/serial",timeout=10)
res["P4_cap"]={"opened":len(socks),"got_503":refused,"serial_while_full":[s,round(ms,1)]}
for so in socks: so.close()
time.sleep(6)
s,_,ms=get("/sync/serial",timeout=10)
res["P4_cap"]["serial_after_close"]=[s,round(ms,1)]
# --- P5 oversize body
big=b"x"*(1024*1024+10)
s,d,ms=get("/settings/view_state","POST",big)
res["P5_big_body"]=[s,round(ms,1)]
# --- P6 curl-style Expect: 100-continue with 1500-byte body
import subprocess
r=subprocess.run(["curl","-s","-o","/dev/null","-w","%{http_code} %{time_total}","-X","POST","-H","Content-Type: application/json","--data-binary",json.dumps({"text":"C"*1500}),f"http://127.0.0.1:{PORT}/settings/signature?account=curl-probe"],capture_output=True,text=True)
res["P6_curl_expect"]=r.stdout
json.dump(res,open(out,"w"),indent=1)
print("probe done",label)
