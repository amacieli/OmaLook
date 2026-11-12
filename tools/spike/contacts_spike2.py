#!/usr/bin/env python3
"""Follow-up to contacts_spike.py: the questions the first run left open (MAPI read-back, delta on the
default folder by id, directory search shapes). Scratch folder only, cleaned up, real data compared."""
import json, sys, pathlib, importlib.util
HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("gs", HERE / "graph_spike.py")
gs = importlib.util.module_from_spec(spec); assert spec and spec.loader; spec.loader.exec_module(gs)
tok, info = gs.access_token(); assert tok, info
g = gs.Graph(tok)
out = []
def note(s): out.append(s); print(s, flush=True)

st, wk = g.call("wk", "GET", "/me/contactFolders/contacts?$select=id,displayName")
dfid = wk["id"]
for label, path in (("delta by default-folder id", f"/me/contactFolders/{dfid}/contacts/delta?$top=2&$select=id"),
                    ("delta via /me/contacts/delta with select", "/me/contacts/delta?$select=id&$top=2")):
    st, d = g.call(label, "GET", path, save=False)
    err = (d or {}).get("error", {}) if isinstance(d, dict) else {}
    note(f"{label}: {st} {err.get('code','')} {str(err.get('message',''))[:150]} nextLink={'@odata.nextLink' in (d or {})} deltaLink={'@odata.deltaLink' in (d or {})} n={len((d or {}).get('value',[])) if st<300 else ''}")

st, f = g.call("mk", "POST", "/me/contactFolders", {"displayName": "omalook-spike-contacts2"}); fid = f["id"]
made = []
try:
    mapi = {"fax": "String 0x3A23", "pager": "String 0x3A21", "car": "String 0x3A1E", "companymain": "String 0x3A57",
            "assistantphone": "String 0x3A2E", "personalhome": "String 0x3A50", "anniversary": "SystemTime 0x3A41", "gender": "Integer 0x3A4D"}
    sv = [{"id": pid, "value": ("2010-06-01T12:00:00Z" if "SystemTime" in pid else ("2" if "Integer" in pid else f"spike-{k}"))} for k, pid in mapi.items()]
    st, c = g.call("create_mapi", "POST", f"/me/contactFolders/{fid}/contacts", {"givenName": "Mapi", "surname": "Two", "singleValueExtendedProperties": sv})
    note(f"create with MAPI: {st} echo has singleValueExtendedProperties: {'singleValueExtendedProperties' in c}")
    cid = c["id"]; made.append(cid)
    for k, pid in mapi.items():
        st, b = g.call("rd", "GET", f"/me/contacts/{cid}?$expand=singleValueExtendedProperties($filter=id eq '{pid}')", save=False)
        got = [x["value"] for x in (b or {}).get("singleValueExtendedProperties", [])] if st < 300 else None
        note(f"- single read {k} {pid}: {st} {got}")
    st, b = g.call("rd_all", "GET", f"/me/contacts/{cid}?$expand=singleValueExtendedProperties($filter=" + " or ".join(f"id eq '{p}'" for p in mapi.values()) + ")", save=False)
    note(f"one read with OR filter: {st} returned {len((b or {}).get('singleValueExtendedProperties', [])) if st<300 else b}")
    st, b = g.call("rd_startswith", "GET", f"/me/contacts/{cid}?$expand=singleValueExtendedProperties($filter=" + " or ".join(f"id eq '{p}'" for p in list(mapi.values())[:2]) + ")", save=False)
    note(f"OR filter of two: {st} returned {len((b or {}).get('singleValueExtendedProperties', [])) if st<300 else b}")
    # list read with expand
    st, l = g.call("list_expand", "GET", f"/me/contactFolders/{fid}/contacts?$expand=singleValueExtendedProperties($filter=id eq '{mapi['fax']}')", save=False)
    note(f"list read with $expand on one property: {st} {[x.get('singleValueExtendedProperties') for x in (l or {}).get('value', [])] if st<300 else ''}")
    # birthday behaviours
    for bd in ("1980-03-03T00:00:00Z", "1980-03-03T12:00:00Z", "1980-03-03"):
        st, r = g.call("bd", "PATCH", f"/me/contacts/{cid}", {"birthday": bd}, save=False)
        note(f"birthday sent {bd!r}: {st} stored {r.get('birthday') if st<300 else str(r)[:120]}")
    # anniversary typed
    # directory search shapes
    for label, path, hd in (
        ("/users $filter startswith", "/users?$filter=startswith(displayName,'a')&$top=3&$select=displayName,mail", {}),
        ("/users $search displayName", "/users?$search=%22displayName%3Aa%22&$top=3&$select=displayName,mail", {"ConsistencyLevel": "eventual"}),
        ("/users plain", "/users?$top=3&$select=displayName,mail", {}),
        ("/me/people search", "/me/people?$search=a&$top=3&$select=displayName", {}),
        ("/me/people search quoted", "/me/people?$search=%22a%22&$top=3", {}),
        ("/me/people with topic", "/me/people?$search=topic%3Aa&$top=3", {}),
        ("/me/people plain top 100", "/me/people?$top=100&$select=displayName,scoredEmailAddresses", {}),
    ):
        st, r = g.call(label, "GET", path, headers=hd, save=False)
        err = (r or {}).get("error", {}) if isinstance(r, dict) else {}
        note(f"{label}: {st} {err.get('code','')} {str(err.get('message',''))[:140]} n={len((r or {}).get('value',[])) if st<300 else ''}")
finally:
    for cid in made: g.call("clean", "POST", f"/me/contacts/{cid}/permanentDelete", save=False)
    st, _ = g.call("cleanfolder", "DELETE", f"/me/contactFolders/{fid}", save=False)
    st, fa = g.call("fa", "GET", "/me/contactFolders?$select=id&$top=50", save=False)
    note(f"cleanup folder {st}; folders left: {len((fa or {}).get('value', []))}")
(HERE.parents[1] / "notes/spike/contacts_findings2.md").write_text("\n".join(out) + "\n")
