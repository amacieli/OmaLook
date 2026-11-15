import http.client, json, os, time, sqlite3
PORT=28155
tokf=f"{os.environ['XDG_RUNTIME_DIR']}/omalook/ui-{PORT}.token"
for _ in range(150):
    if os.path.exists(tokf): break
    time.sleep(0.1)
TOK=open(tokf).read().strip()
c=http.client.HTTPConnection("127.0.0.1",PORT,timeout=20)
c.request("GET","/accounts",headers={"Host":f"127.0.0.1:{PORT}","Authorization":"Bearer "+TOK}); print("accounts",c.getresponse().status)
db=sqlite3.connect(os.environ['XDG_CONFIG_HOME']+"/omalook/messages.db")
with open("/mnt/d/r3-sbx/out/schema.txt","w") as f:
    for name,typ,sql in db.execute("select name,type,sql from sqlite_master where sql is not null order by type,name"):
        f.write(f"-- {typ} {name}\n{sql};\n")
        if typ=='table':
            for r in db.execute(f"pragma table_info('{name}')"):
                f.write(f"   {r[1]} {r[2]} notnull={r[3]} dflt={r[4]} pk={r[5]}\n")
print("schema dumped")
