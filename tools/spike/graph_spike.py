#!/usr/bin/env python3
"""Phase 0 spike for docs/PLAN-calendar-events.md: real answers from Microsoft Graph, nothing guessed.

  graph_spike.py login   device-code sign-in (user enters the code); tries the extended scope set,
                         falls back to the current one and records why. Token file is mode 600,
                         kept OUTSIDE the repo (~/.config/omalook/spike_token.json).
  graph_spike.py probe   read-only probes + self-only write probes (no attendees, ever);
                         every entry it creates is titled "[omalook spike]" and deleted at the end.
  graph_spike.py clean   delete any leftover "[omalook spike]" events.

Raw responses go to notes/spike/ (gitignored: they hold personal data). Findings.md next to them is the summary.
"""
import json, os, sys, time, uuid, urllib.request, urllib.parse, urllib.error, datetime as dt, pathlib

CLIENT = "9c277d6f-edb2-4f82-bda5-901b4c11c457"
G = "https://graph.microsoft.com/"
CURRENT = ["Mail.ReadWrite", "Mail.Send", "Calendars.ReadWrite", "Contacts.ReadWrite", "Tasks.ReadWrite",
           "User.Read", "MailboxSettings.ReadWrite"]
NEW = ["People.Read", "User.ReadBasic.All", "Calendars.Read.Shared"]
TOKEN_FILE = pathlib.Path.home() / ".config/omalook/spike_token.json"
OUT = pathlib.Path(__file__).resolve().parents[2] / "notes/spike"
TAG = "[omalook spike]"
TZ = "Eastern Standard Time"  # Windows name accepted by Graph; IANA also tested in the probe


def post_form(url, data):
    req = urllib.request.Request(url, urllib.parse.urlencode(data).encode())
    try:
        with urllib.request.urlopen(req) as r:
            return r.status, json.load(r)
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


def scopes(names):
    return " ".join(G + n for n in names) + " offline_access"


def login():
    OUT.mkdir(parents=True, exist_ok=True)
    notes = {}
    for label, names in (("extended", CURRENT + NEW), ("current", CURRENT)):
        st, d = post_form("https://login.microsoftonline.com/common/oauth2/v2.0/devicecode",
                          {"client_id": CLIENT, "scope": scopes(names)})
        notes[label + "_devicecode_status"] = st
        if st == 200:
            break
        notes[label + "_error"] = d.get("error_description", d)
        print(f"{label} scope set refused at device-code start: {str(d.get('error_description'))[:200]}", flush=True)
    else:
        print("device-code start failed for both sets"); sys.exit(1)
    print(f"\nSIGN IN: open {d['verification_uri']} and enter code {d['user_code']}\n(scope set: {label})", flush=True)
    end = time.time() + d["expires_in"]
    while time.time() < end:
        time.sleep(d.get("interval", 5))
        st, t = post_form("https://login.microsoftonline.com/common/oauth2/v2.0/token", {
            "grant_type": "urn:ietf:params:oauth:grant-type:device_code", "client_id": CLIENT, "device_code": d["device_code"]})
        if st == 200:
            TOKEN_FILE.write_text(json.dumps({"refresh_token": t["refresh_token"], "scope_set": label}))
            os.chmod(TOKEN_FILE, 0o600)
            notes["granted_scope"] = t.get("scope")
            (OUT / "login.json").write_text(json.dumps(notes, indent=2))
            print("signed in. granted:", t.get("scope"))
            return
        if t.get("error") != "authorization_pending":
            notes["token_error"] = t
            (OUT / "login.json").write_text(json.dumps(notes, indent=2))
            print("sign-in failed:", t.get("error_description", t)); sys.exit(1)
    print("timed out"); sys.exit(1)


def access_token(extra=()):
    saved = json.loads(TOKEN_FILE.read_text())
    names = (CURRENT + NEW) if saved["scope_set"] == "extended" else CURRENT
    st, t = post_form("https://login.microsoftonline.com/common/oauth2/v2.0/token", {
        "grant_type": "refresh_token", "client_id": CLIENT, "refresh_token": saved["refresh_token"],
        "scope": scopes(list(names) + list(extra))})
    if st != 200:
        return None, t
    saved["refresh_token"] = t.get("refresh_token", saved["refresh_token"])
    TOKEN_FILE.write_text(json.dumps(saved))
    return t["access_token"], t


class Graph:
    def __init__(self, tok):
        self.tok = tok
        self.log = []

    def call(self, name, method, path, body=None, headers=None, save=True):
        url = path if path.startswith("http") else G + "v1.0" + path
        url = urllib.parse.quote(url, safe=":/?&=$,()'@")
        h = {"Authorization": "Bearer " + self.tok, "Content-Type": "application/json"}
        h.update(headers or {})
        req = urllib.request.Request(url, json.dumps(body).encode() if body is not None else None, h, method=method)
        t0 = time.time()
        try:
            with urllib.request.urlopen(req) as r:
                raw = r.read()
                status, rh = r.status, dict(r.headers)
        except urllib.error.HTTPError as e:
            raw, status, rh = e.read(), e.code, dict(e.headers)
        try:
            data = json.loads(raw) if raw else None
        except ValueError:
            data = raw.decode(errors="replace")
        rec = {"name": name, "method": method, "path": path, "request": body, "status": status,
               "ms": int((time.time() - t0) * 1000), "response": data}
        if save:
            self.log.append(rec)
        return status, data


def short(d, n=160):
    s = json.dumps(d) if not isinstance(d, str) else d
    return s if len(s) <= n else s[:n] + "..."


def probe():
    OUT.mkdir(parents=True, exist_ok=True)
    summary = []

    def note(line):
        summary.append(line)
        print(line, flush=True)

    tok, info = access_token()
    if not tok:
        print("cannot get a token:", info); sys.exit(1)
    note(f"# Spike run {dt.datetime.now().isoformat(timespec='seconds')}")
    note(f"granted scopes: {info.get('scope')}")
    note("new scopes missing from grant: " + ", ".join(n for n in NEW if n.lower() not in info.get("scope", "").lower()) or "none")
    g = Graph(tok)
    created = []

    st, me = g.call("me", "GET", "/me?$select=displayName,mail,userPrincipalName")
    st2, org = g.call("organization", "GET", "/organization?$select=displayName,verifiedDomains")
    note(f"/me {st}; /organization {st2} ({'work/school tenant' if st2 == 200 and org and org.get('value') else 'no organization => personal or blocked'})")

    # --- calendars and allowed online meeting providers
    st, cals = g.call("calendars", "GET", "/me/calendars?$select=id,name,canEdit,isDefaultCalendar,allowedOnlineMeetingProviders,defaultOnlineMeetingProvider,isTallyingResponses,owner")
    note(f"/me/calendars {st}")
    for c in (cals or {}).get("value", []):
        note(f"  {c['name']!r} default={c.get('isDefaultCalendar')} canEdit={c.get('canEdit')} allowed={c.get('allowedOnlineMeetingProviders')} default_provider={c.get('defaultOnlineMeetingProvider')}")

    st, ms = g.call("mailboxSettings", "GET", "/me/mailboxSettings/workingHours")
    note(f"mailboxSettings/workingHours {st}: {short(ms, 300)}")

    # --- people and directory (scope consent shows here)
    st, p = g.call("people", "GET", "/me/people?$top=5&$select=displayName,scoredEmailAddresses")
    note(f"/me/people {st} ({len((p or {}).get('value', []))} rows)" if st == 200 else f"/me/people {st}: {short(p)}")
    st, u = g.call("users", "GET", "/users?$top=3&$select=displayName,mail")
    note(f"/users {st}" + (f" ({len(u.get('value', []))} rows)" if st == 200 else f": {short(u)}"))
    st, u = g.call("users_filter", "GET", "/users?$filter=startswith(displayName,'a')&$top=3&$select=displayName,mail&$count=true",
                   headers={"ConsistencyLevel": "eventual"})
    note(f"/users?$filter {st}" + ("" if st == 200 else f": {short(u)}"))

    # --- free/busy
    now = dt.datetime.now().replace(minute=0, second=0, microsecond=0) + dt.timedelta(days=1)
    mail = (me or {}).get("mail") or (me or {}).get("userPrincipalName")
    win = {"startTime": {"dateTime": now.isoformat(), "timeZone": TZ},
           "endTime": {"dateTime": (now + dt.timedelta(days=2)).isoformat(), "timeZone": TZ}}
    st, gs = g.call("getSchedule", "POST", "/me/calendar/getSchedule", {"schedules": [mail], "availabilityViewInterval": 30, **win})
    note(f"getSchedule (self) {st}: {short(gs, 200)}")
    st, fm = g.call("findMeetingTimes", "POST", "/me/findMeetingTimes", {
        "attendees": [], "meetingDuration": "PT30M", "timeConstraint": {"timeslots": [{"start": win["startTime"], "end": win["endTime"]}]}, "maxCandidates": 3})
    note(f"findMeetingTimes (no attendees) {st}: {short(fm, 200)}")

    # --- writes: self only, no attendees
    def ev(subject, start, minutes=30, **extra):
        e = {"subject": f"{TAG} {subject}", "start": {"dateTime": start.isoformat(), "timeZone": TZ},
             "end": {"dateTime": (start + dt.timedelta(minutes=minutes)).isoformat(), "timeZone": TZ},
             "isReminderOn": False, "showAs": "free"}
        e.update(extra)
        return e

    def make(name, body, headers=None):
        st, d = g.call(name, "POST", "/me/events", body, headers)
        if st in (200, 201) and d:
            created.append(d["id"])
        return st, d

    base = now + dt.timedelta(days=14)
    st, d = make("create_plain", ev("plain", base))
    note(f"create plain {st}")

    # transactionId behaviour
    tx = str(uuid.uuid4())
    s1, d1 = make("tx_first", ev("transaction", base, transactionId=tx))
    s2, d2 = make("tx_second", ev("transaction", base, transactionId=tx))
    same = d1 and d2 and d1.get("id") == d2.get("id")
    note(f"transactionId: first {s1}, repeat {s2}, same id returned={same}" + ("" if s2 in (200, 201) else f" err={short(d2)}"))
    s3, d3 = make("tx_changed_body", ev("transaction CHANGED", base, transactionId=tx))
    note(f"transactionId reused with a different body: {s3} same id={d3 and d1 and d3.get('id') == d1.get('id')} subject={(d3 or {}).get('subject') if isinstance(d3, dict) else short(d3)}")

    # IANA vs Windows zone names
    s, d = make("tz_iana", ev("iana tz", base) | {"start": {"dateTime": base.isoformat(), "timeZone": "America/New_York"},
                                                  "end": {"dateTime": (base + dt.timedelta(minutes=30)).isoformat(), "timeZone": "America/New_York"}})
    note(f"IANA zone name accepted: {s}" + ("" if s in (200, 201) else f" {short(d)}"))

    # Teams, single
    s, d = make("teams_single", ev("teams single", base + dt.timedelta(hours=2), isOnlineMeeting=True, onlineMeetingProvider="teamsForBusiness"))
    note(f"Teams single {s}: joinUrl={'yes' if isinstance(d, dict) and (d.get('onlineMeeting') or {}).get('joinUrl') else 'no'} provider={(d or {}).get('onlineMeetingProvider') if isinstance(d, dict) else short(d)}")

    # Recurrence
    def rec(pattern, rng):
        return {"recurrence": {"pattern": pattern, "range": rng}}
    start_day = base.date()
    wd = base.strftime("%A").lower()
    ok_range = {"type": "endDate", "startDate": start_day.isoformat(), "endDate": (start_day + dt.timedelta(days=60)).isoformat()}
    s, series = make("rec_weekly_ok", ev("weekly", base + dt.timedelta(hours=4), **rec({"type": "weekly", "interval": 1, "daysOfWeek": [wd]}, ok_range)))
    note(f"weekly series {s}")
    wrong_day = (base + dt.timedelta(days=1)).strftime("%A").lower()
    s, d = make("rec_weekly_start_not_listed_day", ev("weekly wrong day", base + dt.timedelta(hours=5), **rec({"type": "weekly", "interval": 1, "daysOfWeek": [wrong_day]}, ok_range)))
    note(f"weekly where start date's weekday is not listed: {s}" + (f" -> first start {d['start']}" if isinstance(d, dict) and 'start' in d else f" {short(d)}"))
    bad = dict(ok_range, startDate=(start_day + dt.timedelta(days=1)).isoformat())
    s, d = make("rec_range_start_mismatch", ev("range mismatch", base + dt.timedelta(hours=6), **rec({"type": "weekly", "interval": 1, "daysOfWeek": [wd]}, bad)))
    note(f"range.startDate != event start date: {s}" + (f" -> start {d['start']}" if isinstance(d, dict) and 'start' in d else f" {short(d)}"))
    s, d = make("rec_weekdays", ev("weekdays", base + dt.timedelta(hours=7), **rec({"type": "weekly", "interval": 1, "daysOfWeek": ["monday", "tuesday", "wednesday", "thursday", "friday"]}, ok_range)))
    note(f"weekdays pattern {s}")
    s, d = make("rec_relmonthly", ev("2nd tuesday", base + dt.timedelta(hours=8), **rec({"type": "relativeMonthly", "interval": 1, "daysOfWeek": [wd], "index": "second"},
                {"type": "numbered", "startDate": start_day.isoformat(), "numberOfOccurrences": 3})))
    note(f"relativeMonthly numbered {s}" + ("" if s in (200, 201) else f" {short(d)}"))
    s, tseries = make("rec_teams", ev("teams weekly", base + dt.timedelta(hours=9), isOnlineMeeting=True, onlineMeetingProvider="teamsForBusiness",
                                      **rec({"type": "weekly", "interval": 1, "daysOfWeek": [wd]}, ok_range)))
    note(f"Teams on a recurring series {s}: joinUrl={'yes' if isinstance(tseries, dict) and (tseries.get('onlineMeeting') or {}).get('joinUrl') else 'no'}" + ("" if s in (200, 201) else f" {short(tseries)}"))

    # Series occurrences, exception, cancel
    if isinstance(series, dict) and series.get("id"):
        sid = series["id"]
        s, inst = g.call("instances", "GET", f"/me/events/{sid}/instances?startDateTime={start_day}T00:00:00&endDateTime={start_day + dt.timedelta(days=30)}T00:00:00&$select=id,subject,start,type,seriesMasterId")
        occ = (inst or {}).get("value", []) if isinstance(inst, dict) else []
        note(f"instances {s}: {len(occ)} in 30 days; types={sorted({o.get('type') for o in occ})}")
        if len(occ) >= 3:
            s, d = g.call("exception_patch", "PATCH", f"/me/events/{occ[1]['id']}", {"subject": f"{TAG} weekly (moved)",
                          "start": {"dateTime": (base + dt.timedelta(days=7, hours=5)).isoformat(), "timeZone": TZ},
                          "end": {"dateTime": (base + dt.timedelta(days=7, hours=5, minutes=30)).isoformat(), "timeZone": TZ}})
            note(f"PATCH on an occurrence id {s}: type={(d or {}).get('type') if isinstance(d, dict) else short(d)}")
            s, d = g.call("master_patch", "PATCH", f"/me/events/{sid}", {"subject": f"{TAG} weekly (renamed)"})
            note(f"PATCH on master id {s}")
            s, inst2 = g.call("instances_after", "GET", f"/me/events/{sid}/instances?startDateTime={start_day}T00:00:00&endDateTime={start_day + dt.timedelta(days=30)}T00:00:00&$select=id,subject,start,type")
            note("after master rename, exception keeps its own subject: " + short([o["subject"] for o in (inst2 or {}).get("value", [])], 300))
            s, d = g.call("occurrence_delete", "DELETE", f"/me/events/{occ[2]['id']}")
            note(f"DELETE one occurrence {s}")
            s, inst3 = g.call("instances_after_delete", "GET", f"/me/events/{sid}/instances?startDateTime={start_day}T00:00:00&endDateTime={start_day + dt.timedelta(days=30)}T00:00:00&$select=id")
            note(f"instances after delete: {len((inst3 or {}).get('value', []))} (was {len(occ)})")
        # cancel on an event with no attendees
        s, d = g.call("cancel_no_attendees", "POST", f"/me/events/{sid}/cancel", {"comment": "spike"})
        note(f"POST /cancel on a series with no attendees {s}: {short(d, 200)}")

    # dry confirmation that attendees are never used
    note("attendees used in any request: no")

    # --- cleanup
    gone = 0
    for eid in created:
        s, _ = g.call("cleanup", "DELETE", f"/me/events/{eid}", save=False)
        gone += s in (204, 404)
    st, left = g.call("leftovers", "GET", "/me/events?$filter=startswith(subject,'" + TAG + "')&$select=id,subject&$top=50", save=False)
    leftover = [e["id"] for e in (left or {}).get("value", [])] if st == 200 else []
    for eid in leftover:
        g.call("cleanup2", "DELETE", f"/me/events/{eid}", save=False)
    note(f"cleanup: deleted {gone}/{len(created)} created, {len(leftover)} stragglers removed")

    stamp = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    (OUT / f"probe-{stamp}.json").write_text(json.dumps(g.log, indent=1, default=str))
    (OUT / f"findings-{stamp}.md").write_text("\n".join(summary) + "\n")
    print(f"\nraw: {OUT}/probe-{stamp}.json   summary: {OUT}/findings-{stamp}.md")


def clean():
    tok, info = access_token()
    g = Graph(tok)
    st, left = g.call("leftovers", "GET", "/me/events?$filter=startswith(subject,'" + TAG + "')&$select=id,subject&$top=100", save=False)
    for e in (left or {}).get("value", []):
        print(e["subject"], g.call("del", "DELETE", f"/me/events/{e['id']}", save=False)[0])


if __name__ == "__main__":
    {"login": login, "probe": probe, "clean": clean}[sys.argv[1]]()
