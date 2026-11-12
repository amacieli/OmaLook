#!/usr/bin/env python3
"""Phase 0 spike for docs/PLAN-contacts.md: real answers from Graph about contact writes. Nothing guessed.

Uses the separate spike token (~/.config/omalook/spike_token.json, see graph_spike.py login).
Everything is created in a scratch contact folder `omalook-spike-contacts` (and a few scratch
items named omalook-spike-*), then removed with permanentDelete. The real contact folders and
the master category list are compared before and after; the run fails loudly if they differ.
Raw responses go to notes/spike/contacts_raw.json (gitignored: personal data), the summary
to notes/spike/contacts_findings.md.
"""
import base64, json, sys, time, pathlib, importlib.util, datetime as dt

HERE = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("gs", HERE / "graph_spike.py")
gs = importlib.util.module_from_spec(spec); spec.loader.exec_module(gs)
OUT = gs.OUT
TAG = "omalook-spike"

# 1x1 JPEG
JPEG = base64.b64decode("/9j/4AAQSkZJRgABAQEASABIAAD/2wBDAP//////////////////////////////////////////////////////////////////////////////////////wgALCAABAAEBAREA/8QAFBABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQABPxA=")

lines = []
def note(s):
    lines.append(s); print(s, flush=True)

def main():
    OUT.mkdir(parents=True, exist_ok=True)
    tok, info = gs.access_token()
    if not tok:
        print("no token:", info); sys.exit(1)
    g = gs.Graph(tok)
    note(f"# Contacts spike {dt.datetime.now().isoformat(timespec='seconds')}")
    scp = info.get("scope", "")
    note("granted scopes: " + scp)
    for n in ("Contacts.ReadWrite", "People.Read", "User.ReadBasic.All", "MailboxSettings.ReadWrite"):
        note(f"- {n}: {'granted' if n.lower() in scp.lower() else 'NOT granted'}")

    # ---- before snapshot
    st, f0 = g.call("folders_before", "GET", "/me/contactFolders?$select=id,displayName,parentFolderId&$top=200")
    st2, c0 = g.call("count_default_before", "GET", "/me/contacts?$count=true&$top=1&$select=id", headers={"ConsistencyLevel": "eventual"})
    st3, cat0 = g.call("master_before", "GET", "/me/outlook/masterCategories")
    folders_before = sorted(x["displayName"] for x in (f0 or {}).get("value", []))
    cats_before = sorted(x["displayName"] for x in (cat0 or {}).get("value", [])) if st3 == 200 else None
    count_before = (c0 or {}).get("@odata.count")
    note(f"before: folders={len(folders_before)} default-folder contacts={count_before} master categories={None if cats_before is None else len(cats_before)}")

    created_contacts = []   # (path to permanentDelete)
    folder_id = None
    try:
        # ---- folder CRUD
        st, f = g.call("folder_create", "POST", "/me/contactFolders", {"displayName": TAG + "-contacts"})
        note(f"folder create: {st}"); folder_id = f["id"]
        st, f2 = g.call("folder_rename", "PATCH", f"/me/contactFolders/{folder_id}", {"displayName": TAG + "-contacts-renamed"})
        note(f"folder rename: {st}")
        st, ch = g.call("child_create", "POST", f"/me/contactFolders/{folder_id}/childFolders", {"displayName": TAG + "-child"})
        note(f"child folder create: {st}")
        child_id = ch.get("id") if st < 300 else None

        # ---- create with every typed field
        full = {
            "title": "Dr.", "givenName": "Omalook", "middleName": "Q", "surname": "Spike", "generation": "Jr.",
            "initials": "OQS", "nickName": "Spikey", "fileAs": "Spike, Omalook",
            "yomiGivenName": "オマ", "yomiSurname": "スパイク", "yomiCompanyName": "カンパニー",
            "emailAddresses": [{"name": "Omalook Spike", "address": "spike1@example.invalid"},
                               {"name": "Second", "address": "spike2@example.invalid"},
                               {"name": "Third", "address": "spike3@example.invalid"}],
            "imAddresses": ["spike@im.example.invalid"],
            "businessPhones": ["+1 555 0100", "+1 555 0101"], "homePhones": ["+1 555 0200", "+1 555 0201"],
            "mobilePhone": "+1 555 0300",
            "homeAddress": {"street": "1 Home St", "city": "Homeville", "state": "HS", "postalCode": "11111", "countryOrRegion": "USA"},
            "businessAddress": {"street": "2 Biz Ave", "city": "Bizton", "state": "BZ", "postalCode": "22222", "countryOrRegion": "USA"},
            "otherAddress": {"street": "3 Other Rd", "city": "Otherburg", "state": "OT", "postalCode": "33333", "countryOrRegion": "USA"},
            "companyName": "Spike Co", "department": "QA", "jobTitle": "Tester", "officeLocation": "Room 1",
            "profession": "Spiking", "manager": "Boss Spike", "assistantName": "Assist Spike",
            "birthday": "1980-03-03T12:00:00Z", "spouseName": "Spouse Spike", "children": ["Kid A", "Kid B"],
            "personalNotes": "notes line 1\nnotes line 2", "businessHomePage": "https://example.invalid",
            "categories": [TAG + "-nomaster"],
        }
        st, c = g.call("contact_create_full", "POST", f"/me/contactFolders/{folder_id}/contacts", full)
        note(f"create with every typed field: {st}")
        if st >= 300:
            note("  error: " + gs.short(c, 400)); raise SystemExit
        cid = c["id"]; created_contacts.append(f"/me/contacts/{cid}")
        echo = {k: c.get(k) for k in full}
        diff = {k: (full[k], echo[k]) for k in full if full[k] != echo[k]}
        note("fields that came back different from what was sent: " + gs.short(diff, 1200))
        note("response keys not sent: " + ", ".join(sorted(k for k in c if k not in full)))
        note(f"displayName after create: {c.get('displayName')!r}; fileAs: {c.get('fileAs')!r}; has @odata.etag: {'@odata.etag' in c}; changeKey: {bool(c.get('changeKey'))}")
        note(f"primary/secondary/tertiary email keys present: {[k for k in c if 'EmailAddress' in k]}")
        note(f"birthday echo: {c.get('birthday')!r}")

        # ---- PATCH semantics
        st, p1 = g.call("patch_company_only", "PATCH", f"/me/contacts/{cid}", {"companyName": "Spike Co 2"})
        note(f"PATCH companyName only: {st}; displayName now {p1.get('displayName')!r} (was {c.get('displayName')!r}); fileAs {p1.get('fileAs')!r}")
        st, p2 = g.call("patch_given_only", "PATCH", f"/me/contacts/{cid}", {"givenName": "Changed"})
        note(f"PATCH givenName only: {st}; displayName now {p2.get('displayName')!r}; fileAs {p2.get('fileAs')!r}")
        st, p3 = g.call("patch_given_with_display", "PATCH", f"/me/contacts/{cid}", {"givenName": "Changed2", "displayName": "My Chosen Display"})
        note(f"PATCH givenName + displayName: {st}; displayName now {p3.get('displayName')!r}")
        st, p4 = g.call("patch_other_after_display", "PATCH", f"/me/contacts/{cid}", {"jobTitle": "Tester 2"})
        note(f"PATCH jobTitle only after a chosen displayName: displayName now {p4.get('displayName')!r}")
        st, p5 = g.call("patch_other_with_display", "PATCH", f"/me/contacts/{cid}", {"department": "QA2", "displayName": "My Chosen Display"})
        note(f"PATCH department + displayName kept: displayName {p5.get('displayName')!r}")

        # arrays
        st, a1 = g.call("patch_email_one", "PATCH", f"/me/contacts/{cid}", {"emailAddresses": [{"name": "Only", "address": "only@example.invalid"}]})
        note(f"PATCH emailAddresses to one element: {st}; now {[e['address'] for e in a1.get('emailAddresses', [])]}")
        st, a2 = g.call("patch_email_five", "PATCH", f"/me/contacts/{cid}", {"emailAddresses": [{"name": f"E{i}", "address": f"e{i}@example.invalid"} for i in range(5)]})
        note(f"PATCH emailAddresses with five: {st}; now {len(a2.get('emailAddresses', []))} stored; {gs.short(a2 if st >= 300 else '', 300)}")
        st, a3 = g.call("patch_email_no_name", "PATCH", f"/me/contacts/{cid}", {"emailAddresses": [{"address": "noname@example.invalid"}]})
        note(f"PATCH emailAddresses without a name: {st}; stored {gs.short(a3.get('emailAddresses') if st < 300 else a3, 300)}")
        st, a4 = g.call("patch_phones_three", "PATCH", f"/me/contacts/{cid}", {"businessPhones": ["1", "2", "3"], "homePhones": ["4", "5", "6"]})
        note(f"PATCH three business and three home phones: {st}; stored {a4.get('businessPhones') if st < 300 else a4} / {a4.get('homePhones') if st < 300 else ''}")
        st, a5 = g.call("patch_clear_fields", "PATCH", f"/me/contacts/{cid}", {"nickName": "", "children": [], "spouseName": None, "homeAddress": {}})
        note(f"PATCH clear (empty string / [] / null / {{}}): {st}; nickName={a5.get('nickName')!r} children={a5.get('children')!r} spouse={a5.get('spouseName')!r} homeAddress={a5.get('homeAddress')!r}")
        st, a6 = g.call("patch_birthday_nodate", "PATCH", f"/me/contacts/{cid}", {"birthday": "1604-03-03T00:00:00Z"})
        note(f"PATCH birthday with a year-less style date (1604): {st}; stored {a6.get('birthday') if st < 300 else a6}")
        st, a7 = g.call("patch_birthday_null", "PATCH", f"/me/contacts/{cid}", {"birthday": None})
        note(f"PATCH birthday null: {st}; stored {a7.get('birthday') if st < 300 else a7}")

        # ---- If-Match / changeKey
        st, cur = g.call("get_for_etag", "GET", f"/me/contacts/{cid}")
        etag = cur.get("@odata.etag")
        st, w = g.call("patch_bad_ifmatch", "PATCH", f"/me/contacts/{cid}", {"jobTitle": "x"}, headers={"If-Match": 'W/"bogus"'})
        note(f"PATCH with a wrong If-Match: {st} {gs.short((w or {}).get('error', {}).get('code') if isinstance(w, dict) else w, 80)}")
        st, w = g.call("patch_good_ifmatch", "PATCH", f"/me/contacts/{cid}", {"jobTitle": "Tester 3"}, headers={"If-Match": etag})
        note(f"PATCH with the right If-Match: {st}")
        st, w = g.call("patch_stale_ifmatch", "PATCH", f"/me/contacts/{cid}", {"jobTitle": "Tester 4"}, headers={"If-Match": etag})
        note(f"PATCH with the now-stale If-Match: {st} {gs.short((w or {}).get('error', {}).get('code') if isinstance(w, dict) else w, 80)}")

        # ---- delta
        st, d = g.call("delta_folder", "GET", f"/me/contactFolders/{folder_id}/contacts/delta")
        note(f"delta on a folder: {st}; items {len((d or {}).get('value', [])) if st < 300 else ''}; has deltaLink: {'@odata.deltaLink' in (d or {})}; has nextLink: {'@odata.nextLink' in (d or {})}")
        dl = (d or {}).get("@odata.deltaLink")
        st, d0 = g.call("delta_default", "GET", "/me/contacts/delta?$top=1", save=False)
        note(f"delta on the default folder via /me/contacts/delta: {st}")
        if st >= 300:
            st, d0 = g.call("delta_default_folders", "GET", "/me/contactFolders/contacts/contacts/delta?$top=1", save=False)
            note(f"delta on the default folder via /me/contactFolders/contacts/contacts/delta: {st}")
        st, wk = g.call("wellknown", "GET", "/me/contactFolders/contacts?$select=id,displayName")
        note(f"well-known name `contacts` addressable as a folder id: {st} {gs.short(wk, 160)}")
        # change, then follow the delta link
        g.call("patch_for_delta", "PATCH", f"/me/contacts/{cid}", {"department": "after-delta"})
        if dl:
            st, d2 = g.call("delta_follow", "GET", dl)
            note(f"delta link followed after one change: {st}; items {[x.get('id', '')[-6:] + ':' + str(x.get('@removed', 'upd')) for x in (d2 or {}).get('value', [])]}")

        # ---- copy / move
        st, mv = g.call("move_v1", "POST", f"/me/contacts/{cid}/move", {"destinationId": child_id})
        note(f"POST /me/contacts/{{id}}/move (v1.0): {st} {gs.short(mv if st >= 300 else {'newid_differs': mv.get('id') != cid}, 200)}")
        moved_id = cid
        if st < 300 and mv.get("id"):
            moved_id = mv["id"]; created_contacts[-1] = f"/me/contacts/{moved_id}"
        st, mvb = g.call("move_beta", "POST", f"https://graph.microsoft.com/beta/me/contacts/{moved_id}/move", {"destinationId": folder_id})
        note(f"POST beta /me/contacts/{{id}}/move: {st} {gs.short(mvb if st >= 300 else 'ok', 200)}")
        if st < 300 and mvb.get("id"):
            moved_id = mvb["id"]; created_contacts[-1] = f"/me/contacts/{moved_id}"
        # immutable ids
        st, imm = g.call("immutable_get", "GET", f"/me/contacts/{moved_id}?$select=id,parentFolderId", headers={"Prefer": 'IdType="ImmutableId"'})
        note(f"GET with Prefer IdType=ImmutableId: {st}; id differs from the mutable one: {imm.get('id') != moved_id if st < 300 else '?'}")

        # ---- create idempotency / duplicate
        st, dup = g.call("create_dup", "POST", f"/me/contactFolders/{folder_id}/contacts", {"givenName": "Dup", "surname": "Spike", "emailAddresses": [{"address": "dup@example.invalid"}]})
        st2, dup2 = g.call("create_dup2", "POST", f"/me/contactFolders/{folder_id}/contacts", {"givenName": "Dup", "surname": "Spike", "emailAddresses": [{"address": "dup@example.invalid"}]})
        note(f"two identical creates both succeed (no server-side duplicate check): {st} {st2}; ids differ: {dup.get('id') != dup2.get('id') if st < 300 and st2 < 300 else '?'}")
        for x in (dup, dup2):
            if isinstance(x, dict) and x.get("id"): created_contacts.append(f"/me/contacts/{x['id']}")
        # a contact with only an email, no name: displayName?
        st, bare = g.call("create_bare", "POST", f"/me/contactFolders/{folder_id}/contacts", {"emailAddresses": [{"address": "bare@example.invalid"}]})
        note(f"create with only an address: {st}; displayName={bare.get('displayName')!r} fileAs={bare.get('fileAs')!r}")
        if isinstance(bare, dict) and bare.get("id"): created_contacts.append(f"/me/contacts/{bare['id']}")
        st, bare2 = g.call("create_nothing", "POST", f"/me/contactFolders/{folder_id}/contacts", {})
        note(f"create with no fields at all: {st}")
        if isinstance(bare2, dict) and bare2.get("id"): created_contacts.append(f"/me/contacts/{bare2['id']}")

        # ---- categories on contacts, master list untouched
        st, cc = g.call("patch_cats", "PATCH", f"/me/contacts/{moved_id}", {"categories": [TAG + "-a", TAG + "-b"]})
        note(f"PATCH categories with names not in the master list: {st}; stored {cc.get('categories') if st < 300 else cc}")
        st, mc = g.call("contacts_filter_cat", "GET", f"/me/contactFolders/{folder_id}/contacts?$filter=categories/any(c:c eq '{TAG}-a')&$select=id")
        note(f"$filter by category on contacts: {st}; matches {len((mc or {}).get('value', [])) if st < 300 else mc}")
        st, cat1 = g.call("master_mid", "GET", "/me/outlook/masterCategories")
        mid = sorted(x["displayName"] for x in (cat1 or {}).get("value", [])) if st == 200 else None
        note(f"master category list unchanged after tagging contacts with unknown names: {mid == cats_before}")

        # ---- extended (MAPI) properties
        PS = "String {00020329-0000-0000-C000-000000000046} Name "
        mapi = {
            "fax": "String 0x3A23", "pager": "String 0x3A21", "car": "String 0x3A1E", "companymain": "String 0x3A57",
            "assistantphone": "String 0x3A2E", "personalhome": "String 0x3A50", "anniversary": "SystemTime 0x3A41", "gender": "Integer 0x3A4D",
        }
        sv = [{"id": pid, "value": ("2010-06-01T12:00:00Z" if "SystemTime" in pid else ("2" if "Integer" in pid else f"spike-{k}"))} for k, pid in mapi.items()]
        st, ex = g.call("create_mapi", "POST", f"/me/contactFolders/{folder_id}/contacts", {"givenName": "Mapi", "surname": "Spike", "singleValueExtendedProperties": sv})
        note(f"create with MAPI properties (fax, pager, car, company main, assistant phone, personal home page, anniversary, gender): {st} {gs.short(ex if st >= 300 else '', 300)}")
        if st < 300:
            created_contacts.append(f"/me/contacts/{ex['id']}")
            flt = " or ".join(f"id eq '{pid}'" for pid in mapi.values())
            st, back = g.call("read_mapi", "GET", f"/me/contacts/{ex['id']}?$expand=singleValueExtendedProperties($filter={flt})")
            got = {x["id"]: x["value"] for x in (back or {}).get("singleValueExtendedProperties", [])} if st < 300 else {}
            for k, pid in mapi.items():
                note(f"- MAPI {k} ({pid}): {'round-trips' if pid in got else 'NOT returned'} {got.get(pid, '')!r}")
            st, ex2 = g.call("patch_mapi", "PATCH", f"/me/contacts/{ex['id']}", {"singleValueExtendedProperties": [{"id": mapi['fax'], "value": "changed"}]})
            note(f"PATCH one MAPI property: {st}")
            st, back = g.call("read_mapi2", "GET", f"/me/contacts/{ex['id']}?$expand=singleValueExtendedProperties($filter=id eq '{mapi['fax']}')")
            note(f"  fax after PATCH: {[x['value'] for x in (back or {}).get('singleValueExtendedProperties', [])]}")
        # open extension
        st, oe = g.call("open_ext", "POST", f"/me/contacts/{moved_id}/extensions", {"@odata.type": "microsoft.graph.openTypeExtension", "extensionName": "com.omalook.spike", "favorite": True})
        note(f"open extension create: {st}")

        # ---- photo
        st, ph = g.call("photo_put", "PUT", f"/me/contacts/{moved_id}/photo/$value", None, headers={"Content-Type": "image/jpeg"}, save=False)
        # the helper sends JSON; do the photo with urllib directly
        import urllib.request, urllib.error
        req = urllib.request.Request(gs.G + f"v1.0/me/contacts/{moved_id}/photo/$value", JPEG, {"Authorization": "Bearer " + tok, "Content-Type": "image/jpeg"}, method="PUT")
        try:
            with urllib.request.urlopen(req) as r: pst = r.status
        except urllib.error.HTTPError as e: pst = e.code
        note(f"photo PUT (1x1 jpeg): {pst}")
        req = urllib.request.Request(gs.G + f"v1.0/me/contacts/{moved_id}/photo/$value", None, {"Authorization": "Bearer " + tok})
        try:
            with urllib.request.urlopen(req) as r: pst, pb = r.status, r.read()
        except urllib.error.HTTPError as e: pst, pb = e.code, b""
        note(f"photo GET: {pst}, {len(pb)} bytes")
        st, pm = g.call("photo_meta", "GET", f"/me/contacts/{moved_id}/photo")
        note(f"photo metadata: {st} {gs.short(pm, 200)}")

        # ---- directory / people (feeds phase 6)
        st, pp = g.call("people", "GET", "/me/people?$top=3&$select=displayName,scoredEmailAddresses")
        note(f"/me/people: {st} {len((pp or {}).get('value', [])) if st < 300 else gs.short(pp, 200)} entries")
        st, pps = g.call("people_search", "GET", "/me/people?$search=%22a%22&$top=3&$select=displayName")
        note(f"/me/people?$search: {st}")
        st, us = g.call("users_search", "GET", "/users?$search=%22displayName:a%22&$top=3&$select=displayName,mail", headers={"ConsistencyLevel": "eventual"})
        note(f"/users?$search (needs User.ReadBasic.All): {st} {gs.short((us or {}).get('error', {}).get('code') if st >= 300 and isinstance(us, dict) else len((us or {}).get('value', [])), 120)}")

        # ---- delete semantics
        st, dl_ = g.call("delete_normal", "DELETE", f"/me/contacts/{dup['id']}")
        note(f"DELETE contact: {st}")
        created_contacts = [x for x in created_contacts if not x.endswith(dup['id'])]
        st, pd = g.call("permanent_delete", "POST", f"/me/contacts/{dup2['id']}/permanentDelete")
        note(f"permanentDelete contact: {st}")
        created_contacts = [x for x in created_contacts if not x.endswith(dup2['id'])]
        st, listing = g.call("list_folder", "GET", f"/me/contactFolders/{folder_id}/contacts?$select=id,displayName&$top=100")
        note(f"contacts left in the scratch folder: {len((listing or {}).get('value', []))}")
        st, big = g.call("list_page", "GET", f"/me/contactFolders/{folder_id}/contacts?$top=2&$select=id")
        note(f"paging works with $top (nextLink present with 2 per page): {'@odata.nextLink' in (big or {})}")
    finally:
        # ---- cleanup (never leave scratch data)
        for path in created_contacts:
            st, _ = g.call("cleanup_contact", "POST", path + "/permanentDelete", save=False)
        if folder_id:
            for fid in [folder_id]:
                st, _ = g.call("cleanup_folder", "DELETE", f"/me/contactFolders/{fid}", save=False)
                note(f"cleanup: scratch folder delete {st}")
        st, left = g.call("cleanup_scan", "GET", "/me/contacts?$top=50&$select=id,displayName&$search=%22omalook-spike%22", headers={"ConsistencyLevel": "eventual"}, save=False)
        st, fa = g.call("folders_after", "GET", "/me/contactFolders?$select=id,displayName,parentFolderId&$top=200")
        st2, ca = g.call("count_default_after", "GET", "/me/contacts?$count=true&$top=1&$select=id", headers={"ConsistencyLevel": "eventual"})
        st3, cat2 = g.call("master_after", "GET", "/me/outlook/masterCategories")
        folders_after = sorted(x["displayName"] for x in (fa or {}).get("value", []))
        cats_after = sorted(x["displayName"] for x in (cat2 or {}).get("value", [])) if st3 == 200 else None
        count_after = (ca or {}).get("@odata.count")
        ok = folders_before == folders_after and count_before == count_after and cats_before == cats_after
        note(f"after: folders {len(folders_after)} (same as before: {folders_before == folders_after}); default contacts {count_after} (same: {count_before == count_after}); master categories same: {cats_before == cats_after}")
        note("RESULT: real data identical before and after" if ok else "RESULT: *** DIFFERENCE: check by hand ***")
        (OUT / "contacts_raw.json").write_text(json.dumps(g.log, indent=1, default=str))
        (OUT / "contacts_findings.md").write_text("\n".join(lines) + "\n")
        if not ok:
            sys.exit(2)


if __name__ == "__main__":
    main()
