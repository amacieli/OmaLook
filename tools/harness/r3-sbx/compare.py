# compare.py old.jsonl new.jsonl — prints requests whose status or body differ.
import json,sys
a=[json.loads(l) for l in open(sys.argv[1])]; b=[json.loads(l) for l in open(sys.argv[2])]
diff=0
for x,y in zip(a,b):
    def scrub(v):
        # environment noise: binary path in /settings/mcp, file-mtime serials
        if isinstance(v,dict): return {k:("*" if k in ("command","serial","updated_at","created_at") else scrub(x)) for k,x in v.items()}
        if isinstance(v,list): return [scrub(x) for x in v]
        return v
    def norm(t):
        t=t.replace("/mnt/d/r3-sbx/omalook-old","BIN").replace("/mnt/d/r3-sbx/bin/omalook","BIN")
        try: return scrub(json.loads(t))
        except Exception: return t
    if x["status"]!=y["status"] or norm(x["body"])!=norm(y["body"]):
        diff+=1; print(f"DIFF {x['m']} {x['p']}: old {x['status']} {x['body'][:150]!r}\n     new {y['status']} {y['body'][:150]!r}")
print(f"compared {min(len(a),len(b))}, differing {diff}")
