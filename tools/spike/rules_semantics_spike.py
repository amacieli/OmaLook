#!/usr/bin/env python3
"""OL-040 phase 4 gate: do Graph inbox rules AND / OR the way the local engine assumes? (docs/PLAN-tags-and-rules.md)

Creates ENABLED scratch rules named omalook-spike-sem-*, tag-only (assignCategories on scratch tag names, plus
stopProcessingRules for the stop test), at the END of the rule list; sends test messages from the account to itself
(not saved to Sent Items) with a unique run token in subject/body; reads back which scratch tags each message got;
then deletes the rules and the test messages and proves the rule list is unchanged. No move/delete/forward actions.

  rules_semantics_spike.py run
"""
import sys, os, json, time, uuid, hashlib, pathlib
sys.path.insert(0, os.path.dirname(__file__))
import graph_spike as gs
import tags_spike as ts

NAME = "omalook-spike-sem-"
OUT = gs.OUT


def sig(rule_list):
    return sorted((r["id"], hashlib.sha256(json.dumps(r, sort_keys=True).encode()).hexdigest()[:12]) for r in rule_list)


def main():
    g, _ = ts.graph()
    st, me = g.call("me", "GET", "/me?$select=mail,userPrincipalName")
    addr = (me.get("mail") or me.get("userPrincipalName"))
    st, before = ts.rules(g)
    assert st == 200, before
    before_sig = sig(before)
    print(f"rules before: {len(before)}")

    R = "ZQ" + uuid.uuid4().hex[:8].upper()
    tag = lambda n: f"omalook-spike-sem-{n}"
    # (message key, subject, body, expected scratch tags)
    msgs = [
        ("A_subject_only", f"{R}A hello", "plain text", []),                      # AND: one predicate of two
        ("A_body_only",    "hello", f"plain {R}B",       []),                      # AND: the other one
        ("A_both",         f"{R}A hello", f"plain {R}B", ["A"]),                  # AND: both
        ("B_second_value", f"{R}Y hello", "plain",       ["B"]),                  # OR across values of one predicate
        ("C_alone",        f"{R}C hello", "plain",       ["C"]),                  # exceptions: none present
        ("C_exc_body",     f"{R}C hello", f"plain {R}E1", []),                    # exception 1 only
        ("C_exc_subject",  f"{R}C {R}E2", "plain",       []),                     # exception 2 only (any one is enough)
        ("D_stop",         f"{R}D hello", "plain",       ["D1"]),                 # stop: second rule must not run
        ("none",           f"hello {R}N", "plain nothing", []),                      # nothing matches
    ]
    rule_defs = [
        ("A", {"subjectContains": [f"{R}A"], "bodyContains": [f"{R}B"]}, {}, [tag("A")], False),
        ("B", {"subjectContains": [f"{R}X", f"{R}Y"]}, {}, [tag("B")], False),
        ("C", {"subjectContains": [f"{R}C"]}, {"bodyContains": [f"{R}E1"], "subjectContains": [f"{R}E2"]}, [tag("C")], False),
        ("D1", {"subjectContains": [f"{R}D"]}, {}, [tag("D1")], True),
        ("D2", {"subjectContains": [f"{R}D"]}, {}, [tag("D2")], False),
    ]

    # Safety pre-check: no existing enabled rule may match what we send (they sit earlier and most stop).
    texts = (addr + " " + " ".join(m[1] + " " + m[2] for m in msgs)).lower()
    for r in before:
        if not r.get("isEnabled"):
            continue
        for sect in ("conditions",):
            for k, v in (r.get(sect) or {}).items():
                vals = v if isinstance(v, list) else []
                for x in vals:
                    s = x if isinstance(x, str) else (x.get("emailAddress", {}).get("address", ""))
                    if s and s.lower() in texts:
                        print(f"ABORT: existing rule {r.get('displayName')!r} would match test mail ({k})"); sys.exit(2)
    print("pre-check ok: no existing rule matches the test mail")

    created, sent_ok = [], False
    try:
        for key, cond, exc, tags, stop in rule_defs:
            body = {"displayName": NAME + key, "sequence": 9999, "isEnabled": True, "conditions": cond,
                    "actions": {"assignCategories": tags, "stopProcessingRules": stop}}
            if exc:
                body["exceptions"] = exc
            st, d = g.call("create " + key, "POST", "/me/mailFolders/inbox/messageRules", body)
            if st not in (200, 201):
                print("create failed", key, st, gs.short(d, 300)); raise SystemExit(3)
            created.append(d["id"])
        print(f"created {len(created)} enabled scratch rules")
        time.sleep(3)
        for key, subj, body, _ in msgs:
            st, d = g.call("send " + key, "POST", "/me/sendMail", {"message": {
                "subject": subj, "body": {"contentType": "Text", "content": body},
                "toRecipients": [{"emailAddress": {"address": addr}}]}, "saveToSentItems": False})
            if st != 202:
                print("send failed", key, st, gs.short(d, 300)); raise SystemExit(4)
        sent_ok = True
        print(f"sent {len(msgs)} test messages, run token {R}; waiting for delivery")
        got = {}
        for _ in range(40):
            time.sleep(5)
            st, d = g.call("poll", "GET", "/me/mailFolders/inbox/messages?$top=50&$orderby=receivedDateTime desc&$select=id,subject,categories,bodyPreview", save=False)
            if st != 200:
                continue
            got = {m["id"]: m for m in d["value"] if R in (m["subject"] or "") or R in (m.get("bodyPreview") or "")}
            if len(got) >= len(msgs):
                break
        time.sleep(15)  # rules run at delivery; give any late processing a chance
        st, d = g.call("final", "GET", "/me/mailFolders/inbox/messages?$top=50&$orderby=receivedDateTime desc&$select=id,subject,categories,bodyPreview")
        got = {m["id"]: m for m in d["value"] if R in (m["subject"] or "") or R in (m.get("bodyPreview") or "")}
        print(f"delivered to Inbox: {len(got)} of {len(msgs)}")
        # unambiguous match on (subject, body) instead
        by_key = {}
        for key, subj, body, _ in msgs:
            for m in got.values():
                if (m["subject"] or "") == subj and (m.get("bodyPreview") or "").strip() == body.strip():
                    by_key[key] = m
        results, ok_all = [], True
        for key, subj, body, expect in msgs:
            m = by_key.get(key)
            if not m:
                results.append((key, expect, None, "NOT DELIVERED")); ok_all = False; continue
            have = sorted(c.replace("omalook-spike-sem-", "") for c in (m.get("categories") or []) if c.startswith("omalook-spike-sem-"))
            verdict = "ok" if have == sorted(expect) else "MISMATCH"
            ok_all &= verdict == "ok"
            results.append((key, expect, have, verdict))
        print(f"{'case':16} {'expected':10} {'got':10} verdict")
        for key, e, h, v in results:
            print(f"{key:16} {str(e):10} {str(h):10} {v}")
        controls = [r for r in results if r[0] in ("A_both", "C_alone")]
        if all(r[2] == [] or r[2] is None for r in controls):
            print("INCONCLUSIVE: positive controls were not tagged at all (rules may not run on self-sent mail)")
        OUT.mkdir(parents=True, exist_ok=True)
        (OUT / "rules-semantics.json").write_text(json.dumps({"token": R, "results": results, "ok": ok_all}, indent=1))
        print("ALL AS EXPECTED" if ok_all else "NOT ALL AS EXPECTED")
    finally:
        for rid in created:
            st, _ = g.call("del rule", "DELETE", f"/me/mailFolders/inbox/messageRules/{rid}")
            if st != 204:
                print("rule delete status", rid, st)
        # remove test messages: Inbox, then the Deleted Items copies
        for folder in ("inbox", "deleteditems"):
            for _ in range(2):
                st, d = g.call("find " + folder, "GET", f"/me/mailFolders/{folder}/messages?$top=50&$orderby=receivedDateTime desc&$select=id,subject,bodyPreview", save=False)
                if st != 200:
                    break
                for m in d["value"]:
                    if R in (m["subject"] or "") or R in (m.get("bodyPreview") or ""):
                        g.call("del msg", "DELETE", f"/me/messages/{m['id']}", save=False)
                time.sleep(2)
        st, after = ts.rules(g)
        same = st == 200 and sig(after) == before_sig
        print(f"rules after: {len(after) if st == 200 else '?'}; identical to before: {same}")
        left = []
        for folder in ("inbox", "deleteditems"):
            st, d = g.call("verify " + folder, "GET", f"/me/mailFolders/{folder}/messages?$top=50&$orderby=receivedDateTime desc&$select=id,subject,bodyPreview", save=False)
            if st == 200:
                left += [m for m in d["value"] if R in (m["subject"] or "") or R in (m.get("bodyPreview") or "")]
        print(f"test messages left in Inbox/Deleted Items: {len(left)}")


if __name__ == "__main__":
    if sys.argv[1:] == ["run"]:
        main()
    else:
        print(__doc__)
