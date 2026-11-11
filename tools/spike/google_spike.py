#!/usr/bin/env python3
"""Phase 0 Google half for docs/PLAN-tags-and-rules.md (OL-040). Scratch account gmail-2a8d9f only.

  google_spike.py login   browser sign-in (loopback redirect) for the scratch account with the scopes the spike needs
                          (gmail.settings.basic, gmail.labels, calendar.events). Uses the app's google_client.json.
                          Token kept OUTSIDE the repo (~/.config/omalook/spike_google_token.json, mode 600); the app's
                          own stored tokens are not touched. Prints the consent URL; records scopes actually granted.
  google_spike.py delivery  (OL-040 phase 5) filters in the gmail_filters.rs shapes, one self-sent message, which labels land.
  google_spike.py probe   filters: list before, create scratch label + filter (scratch sender), list, try a PATCH-like
                          edit (expected: none), delete both, list after (must equal before).
                          calendar: create event on primary with extendedProperties.private, read back, patch, delete.
"""
import json, os, sys, time, secrets, threading, http.server, urllib.request, urllib.parse, urllib.error, pathlib, datetime as dt, webbrowser

ACCOUNT = "teamcisconc300@gmail.com"
CFG = pathlib.Path.home() / ".config/omalook"
TOKEN_FILE = CFG / "spike_google_token.json"
OUT = pathlib.Path(__file__).resolve().parents[2] / "notes/spike"
SCOPES = ["openid", "email",
          "https://www.googleapis.com/auth/gmail.settings.basic",
          "https://www.googleapis.com/auth/gmail.labels",
          "https://www.googleapis.com/auth/calendar.events",
          "https://www.googleapis.com/auth/gmail.modify"]  # delivery spike: send to self, read labels, trash
SCRATCH = "omalook-spike-nobody@example.invalid"
LABEL = "omalook-spike-label"


def client():
    j = json.load(open(CFG / "google_client.json"))
    j = j.get("installed") or j.get("web") or j
    return j["client_id"], j["client_secret"]


def post_form(url, data):
    req = urllib.request.Request(url, urllib.parse.urlencode(data).encode())
    try:
        with urllib.request.urlopen(req) as r:
            return r.status, json.load(r)
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


def login():
    cid, secret = client()
    state = secrets.token_urlsafe(16)
    got = {}

    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            q = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
            if "code" in q or "error" in q:
                got.update({k: v[0] for k, v in q.items()})
            self.send_response(200); self.end_headers()
            self.wfile.write(b"omalook spike: you can close this tab.")
        def log_message(self, *a): pass

    srv = http.server.HTTPServer(("127.0.0.1", 0), H)
    redirect = f"http://127.0.0.1:{srv.server_port}"
    url = "https://accounts.google.com/o/oauth2/v2/auth?" + urllib.parse.urlencode({
        "client_id": cid, "redirect_uri": redirect, "response_type": "code", "scope": " ".join(SCOPES),
        "access_type": "offline", "prompt": "consent", "state": state, "login_hint": ACCOUNT})
    print(f"\nSIGN IN as {ACCOUNT}:\n{url}\n", flush=True)
    try: webbrowser.open(url)
    except Exception: pass
    end = time.time() + 300
    srv.timeout = 2
    while time.time() < end and "code" not in got and "error" not in got:
        srv.handle_request()
    if "error" in got or "code" not in got:
        print("sign-in failed:", got or "timeout"); sys.exit(1)
    if got.get("state") != state:
        print("state mismatch"); sys.exit(1)
    st, t = post_form("https://oauth2.googleapis.com/token", {
        "code": got["code"], "client_id": cid, "client_secret": secret,
        "redirect_uri": redirect, "grant_type": "authorization_code"})
    if st != 200:
        print("token exchange failed", st, t); sys.exit(1)
    TOKEN_FILE.write_text(json.dumps({"refresh_token": t["refresh_token"]}))
    os.chmod(TOKEN_FILE, 0o600)
    print("granted scopes:", t.get("scope"))
    # confirm which account signed in
    req = urllib.request.Request("https://openidconnect.googleapis.com/v1/userinfo",
                                 headers={"Authorization": "Bearer " + t["access_token"]})
    email = json.load(urllib.request.urlopen(req)).get("email")
    print("signed in as:", email)
    if email != ACCOUNT:
        TOKEN_FILE.unlink()
        print("WRONG ACCOUNT, token discarded"); sys.exit(1)


def access():
    cid, secret = client()
    st, t = post_form("https://oauth2.googleapis.com/token", {
        "client_id": cid, "client_secret": secret, "grant_type": "refresh_token",
        "refresh_token": json.load(open(TOKEN_FILE))["refresh_token"]})
    assert st == 200, (st, t)
    return t["access_token"], t.get("scope")


def call(tok, method, url, body=None):
    req = urllib.request.Request(url, method=method, data=json.dumps(body).encode() if body is not None else None,
                                 headers={"Authorization": "Bearer " + tok, "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req) as r:
            raw = r.read()
            return r.status, (json.loads(raw) if raw else {})
    except urllib.error.HTTPError as e:
        raw = e.read()
        try: return e.code, json.loads(raw)
        except Exception: return e.code, {"raw": raw[:300].decode(errors="replace")}


def probe():
    OUT.mkdir(parents=True, exist_ok=True)
    tok, scope = access()
    res = {"scope": scope}
    G = "https://gmail.googleapis.com/gmail/v1/users/me/"
    st, before = call(tok, "GET", G + "settings/filters")
    res["filters_list_status"] = st
    ids_before = sorted(f["id"] for f in before.get("filter", []))
    res["filters_before"] = len(ids_before)
    st, lab = call(tok, "POST", G + "labels", {"name": LABEL, "labelListVisibility": "labelShow", "messageListVisibility": "show"})
    res["label_create"] = st
    lid = lab.get("id")
    fid = None
    if lid:
        st, f = call(tok, "POST", G + "settings/filters",
                     {"criteria": {"from": SCRATCH}, "action": {"addLabelIds": [lid]}})
        res["filter_create"] = st
        fid = f.get("id")
        if fid:
            st, got = call(tok, "GET", G + f"settings/filters/{fid}")
            res["filter_get"] = {"status": st, "criteria": got.get("criteria"), "action_has_label": lid in got.get("action", {}).get("addLabelIds", [])}
            st, _ = call(tok, "PUT", G + f"settings/filters/{fid}", {"criteria": {"from": SCRATCH}})
            res["filter_edit_PUT_status"] = st
            st, _ = call(tok, "PATCH", G + f"settings/filters/{fid}", {"criteria": {"from": SCRATCH}})
            res["filter_edit_PATCH_status"] = st
        else:
            res["filter_create_body"] = f
    if fid:
        res["filter_delete"] = call(tok, "DELETE", G + f"settings/filters/{fid}")[0]
    if lid:
        res["label_delete"] = call(tok, "DELETE", G + f"labels/{lid}")[0]
    st, after = call(tok, "GET", G + "settings/filters")
    res["filters_after_equal"] = sorted(f["id"] for f in after.get("filter", [])) == ids_before

    # calendar extendedProperties.private round trip
    C = "https://www.googleapis.com/calendar/v3/calendars/primary/events"
    start = (dt.datetime.utcnow() + dt.timedelta(days=30)).replace(minute=0, second=0, microsecond=0)
    ev = {"summary": "[omalook spike] tags", "start": {"dateTime": start.isoformat() + "Z"},
          "end": {"dateTime": (start + dt.timedelta(hours=1)).isoformat() + "Z"},
          "extendedProperties": {"private": {"omalook_tags": "Family,SaintLucia"}}}
    st, e = call(tok, "POST", C, ev)
    res["event_create"] = st
    eid = e.get("id")
    if eid:
        st, g = call(tok, "GET", f"{C}/{eid}")
        res["event_private_roundtrip"] = g.get("extendedProperties", {}).get("private")
        st, p = call(tok, "PATCH", f"{C}/{eid}", {"extendedProperties": {"private": {"omalook_tags": "Family"}}})
        res["event_patch"] = {"status": st, "private": p.get("extendedProperties", {}).get("private")}
        st, q = call(tok, "GET", C + "?" + urllib.parse.urlencode({"privateExtendedProperty": "omalook_tags=Family"}))
        res["event_filter_by_private_prop"] = {"status": st, "found": any(i["id"] == eid for i in q.get("items", []))}
        res["event_delete"] = call(tok, "DELETE", f"{C}/{eid}")[0]
    (OUT / "google-tags-spike.json").write_text(json.dumps(res, indent=2))
    print(json.dumps(res, indent=2))


def delivery():
    import base64, email.message
    OUT.mkdir(parents=True, exist_ok=True)
    tok, scope = access()
    G = "https://gmail.googleapis.com/gmail/v1/users/me/"
    me = ACCOUNT
    run = secrets.token_hex(3)
    # name -> (criteria, should the message get the label?)
    cases = {
        "A-address":      ({"from": me}, True),
        "B-at-domain":    ({"from": "@gmail.com"}, True),
        "C-bare-domain":  ({"from": "gmail.com"}, True),
        "D-or-values":    ({"from": "nobody@example.invalid OR " + me}, True),
        "E-exception":    ({"from": me, "negatedQuery": f"from:({me})"}, False),
        "F-control":      ({"from": "nobody@example.invalid"}, False),
        "G-two-senders":  ({"from": me, "query": "from:(@gmail.com)"}, True),
        "H-to":           ({"to": me}, True),
        "I-or-exc-miss":  ({"from": me, "negatedQuery": "from:(nobody@example.invalid) OR to:(@example.invalid)"}, True),
        "J-attach-true":  ({"from": me, "hasAttachment": True}, False),   # the message has no attachment
        "K-partial-word": ({"from": "teamcisco"}, None),                  # substring of the address: what does Gmail do?
    }
    res = {"run": run, "scope": scope, "cases": {}}
    made = {}
    for name, (crit, _) in cases.items():
        st, lab = call(tok, "POST", G + "labels", {"name": f"omalook-spike-{run}-{name}", "labelListVisibility": "labelShow", "messageListVisibility": "show"})
        if st != 200: res["cases"][name] = {"label_create": st, "body": lab}; continue
        st, f = call(tok, "POST", G + "settings/filters", {"criteria": crit, "action": {"addLabelIds": [lab["id"]]}})
        res["cases"][name] = {"filter_create": st, "criteria": crit}
        if st == 200:
            st2, got = call(tok, "GET", G + f"settings/filters/{f['id']}")
            res["cases"][name]["stored_criteria"] = got.get("criteria")
        made[name] = (lab["id"], f.get("id"))
    # send one message to self
    m = email.message.EmailMessage()
    m["To"] = me; m["From"] = me; m["Subject"] = f"omalook spike {run}"
    m.set_content("omalook filter delivery spike; safe to delete")
    raw = base64.urlsafe_b64encode(m.as_bytes()).decode()
    st, sent = call(tok, "POST", G + "messages/send", {"raw": raw})
    res["send"] = st
    mid = sent.get("id"); got = None
    for _ in range(12):
        time.sleep(5)
        st, q = call(tok, "GET", G + "messages?" + urllib.parse.urlencode({"q": f"subject:{run}", "includeSpamTrash": "true"}))
        # the sent copy and the received copy are two messages; take the one in INBOX
        for it in q.get("messages", []):
            st, g = call(tok, "GET", G + f"messages/{it['id']}?format=minimal")
            if "INBOX" in g.get("labelIds", []): got = g
        if got: break
    res["received"] = bool(got)
    if got:
        ids = set(got["labelIds"])
        for name, (lid, _) in made.items():
            res["cases"][name]["labelled"] = lid in ids
            res["cases"][name]["expected"] = cases[name][1]
    # clean up everything made
    for name, (lid, fid) in made.items():
        if fid: call(tok, "DELETE", G + f"settings/filters/{fid}")
        call(tok, "DELETE", G + f"labels/{lid}")
    if mid:
        for it in (q.get("messages", []) if got is not None or True else []):
            call(tok, "POST", G + f"messages/{it['id']}/trash")
    st, after = call(tok, "GET", G + "settings/filters")
    res["filters_left"] = len(after.get("filter", []))
    (OUT / "google-filter-delivery-spike.json").write_text(json.dumps(res, indent=2))
    print(json.dumps(res, indent=2))


if __name__ == "__main__":
    {"login": login, "probe": probe, "delivery": delivery}[sys.argv[1]]()
