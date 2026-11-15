
import json, sys, time, http.client
out=sys.argv[1]
def req(path, method="GET", body=None):
    c=http.client.HTTPConnection("127.0.0.1",27182,timeout=15)
    try:
        c.request(method,path,body=body,headers={"Content-Type":"application/json"} if body else {})
        r=c.getresponse(); return [r.status, r.read().decode("utf8","replace")[:400]]
    except Exception as e: return [-1,str(e)]
    finally: c.close()
for i in range(150):
    if req("/sync/serial")[0]==200: break
    time.sleep(0.2)
req("/accounts")
res={}
res["compose_send_empty"]=req("/compose/send","POST",json.dumps({"account_id":"","to":[],"subject":"x","body":{"format":"text","content":"hi"}}))
res["drafts_save"]=req("/drafts/save?state=editing","POST",json.dumps({"account_id":"","to":[],"subject":"probe draft","body":{"format":"text","content":"hi"}}))
res["drafts_list"]=req("/drafts")
res["view_state_post"]=req("/settings/view_state","POST",json.dumps({"probe":1}))
res["view_state_get"]=req("/settings/view_state")
res["mail_settings_post"]=req("/settings/mail","POST",json.dumps({"send_delay":7}))
res["mail_settings_get"]=req("/settings/mail")
res["contacts_quick"]=req("/contacts/quick","POST",json.dumps({"account_id":"","name":"A","address":"a@example.com"}))
res["sms_send_empty"]=req("/sms/send","POST",json.dumps({"to":"","text":""}))
res["calendar_event_dry"]=req("/calendar/event?dry_run=1","POST",json.dumps({"subject":"x"}))
res["search_history_post"]=req("/search/history","POST",json.dumps(["probe"]))
res["search_history_get"]=req("/search/history")
res["list_width"]=req("/settings/list_width_offset","POST","12")
res["ui"]=req("/settings/ui")
res["perf"]=req("/perf?m=probe","POST")
json.dump(res,open(out,"w"),indent=1)
