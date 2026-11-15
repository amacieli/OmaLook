
import json, sys, time, http.client, subprocess, os
out=sys.argv[1]; label=sys.argv[2]; BIN=sys.argv[3]
EVIL={"Origin":"https://evil.example"}
def req(path, method="GET", body=None, headers=None, host=None):
    c=http.client.HTTPConnection("127.0.0.1",27182,timeout=10)
    try:
        c.putrequest(method,path,skip_host=True,skip_accept_encoding=True)
        c.putheader("Host", host or "127.0.0.1:27182")
        for k,v in (headers or {}).items(): c.putheader(k,v)
        if body is not None: c.putheader("Content-Length",str(len(body)))
        c.endheaders(body.encode() if isinstance(body,str) else body)
        r=c.getresponse(); d=r.read().decode("utf8","replace")
        return {"s":r.status,"acao":r.getheader("Access-Control-Allow-Origin"),"b":d[:160]}
    except Exception as e: return {"s":-1,"b":str(e)[:100]}
    finally: c.close()
for i in range(150):
    if req("/sync/serial")["s"]==200: break
    time.sleep(0.2)
req("/accounts")
r={}
r["ui_get_accounts"]=req("/accounts")
r["ui_post_view_state"]=req("/settings/view_state","POST",'{"a":1}',{"Content-Type":"text/plain;charset=UTF-8"})
r["P1_origin_evil_accounts"]=req("/accounts",headers=EVIL)
r["P1b_origin_null"]=req("/accounts",headers={"Origin":"null"})
r["P1c_origin_self"]=req("/accounts",headers={"Origin":"http://127.0.0.1:27182"})
r["P2_host_attacker"]=req("/folders",host="attacker.example")
r["P2b_host_localhost"]=req("/folders",host="localhost:27182")
r["P2c_no_host"]=req("/folders",host="")
r["P3_fs_list_origin"]=req("/fs/list?path=/etc&hidden=true",headers=EVIL)
r["P3b_fs_list_nocors_fetchmeta"]=req("/fs/list?path=/etc",headers={"Sec-Fetch-Site":"cross-site","Sec-Fetch-Mode":"no-cors","Sec-Fetch-Dest":"image"})
r["P4_compose_send_textplain_origin"]=req("/compose/send","POST",'{"account_id":"x","to":[],"subject":"x","body":"x"}',{**EVIL,"Content-Type":"text/plain;charset=UTF-8"})
for q in ["enabled?value=true","domain?domain=mail&level=write","tool?name=mail_send&mode=allow","token/regenerate","copy?what=token"]:
    r["P5_mcp_"+q]=req("/settings/mcp/"+q,"POST",None,EVIL)
r["P5_mcp_after"]=req("/settings/mcp")["b"][:120]
r["P5b_senders_add"]=req("/settings/senders/add?email=attacker@evil.example","POST",None,EVIL)
r["P5b_senders_after"]=req("/settings/senders")["b"]
r["P9_gmail_login_origin"]=req("/auth/login?provider=gmail","POST",None,EVIL)
time.sleep(1)
r["P9_xdg_open_log"]=open("/mnt/d/r12-sbx/out/stubs.log").read()[-200:] if os.path.exists("/mnt/d/r12-sbx/out/stubs.log") else ""
r["P11_accounts_remove_origin"]=req("/accounts/remove?account=exchange-x","POST",None,EVIL)
r["signature_origin"]=req("/settings/signature?account=a","POST",'{"text":"hi <a href=https://evil.example>x</a>"}',{**EVIL,"Content-Type":"text/plain"})
r["signature_after"]=req("/settings/signatures")["b"]
r["any_acao_header"]=any(v.get("acao") for v in r.values() if isinstance(v,dict))
# second launch while this backend runs: must detect it (background::backend_listening) and not try to bind
env=dict(os.environ); 
p=subprocess.run(["timeout","8",BIN],capture_output=True,text=True,env=env)
log=(p.stdout+p.stderr)
r["second_launch"]={"detected":"already running" in log, "bind_failed":"BIND FAILED" in log, "tail":[l for l in log.splitlines() if "running" in l or "BIND" in l][:3]}
json.dump(r,open(out,"w"),indent=1)
