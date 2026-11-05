#!/usr/bin/env python3
"""Phase 0 spike for docs/PLAN-tags-and-rules.md (OL-040). Reuses graph_spike.py's sign-in and token file.

  tags_spike.py rules    READ ONLY: list Inbox messageRules, masterCategories; summarise to notes/spike/tags-rules.json
                         (gitignored; holds personal data) and print a summary with no addresses.
  tags_spike.py scratch  writes: a DISABLED rule "omalook-spike-rule" and a category "omalook-spike-cat" on the
                         account; round trips them and deletes both; lists rules/categories before and after
                         and fails loudly if anything else changed.
"""
import json, sys, collections
sys.path.insert(0, __import__("os").path.dirname(__file__))
import graph_spike as gs

RULE = "omalook-spike-rule"
CAT = "omalook-spike-cat"


def graph():
    tok, info = gs.access_token()
    if not tok:
        print("cannot get a token:", info); sys.exit(1)
    return gs.Graph(tok), info


def all_pages(g, name, path):
    out, url = [], path
    while url:
        st, d = g.call(name, "GET", url)
        if st != 200:
            return st, d
        out += d.get("value", [])
        url = d.get("@odata.nextLink")
    return 200, out


def rules(g):
    return all_pages(g, "rules", "/me/mailFolders/inbox/messageRules")


def cats(g):
    return all_pages(g, "cats", "/me/outlook/masterCategories")


def read_only():
    g, info = graph()
    print("granted scopes:", info.get("scope"))
    st, rs = rules(g)
    if st != 200:
        print("messageRules failed", st, gs.short(rs, 400)); return
    st2, cs = cats(g)
    gs.OUT.mkdir(parents=True, exist_ok=True)
    (gs.OUT / "tags-rules.json").write_text(json.dumps({"rules": rs, "categories": cs if st2 == 200 else str(cs)}, indent=2))
    cond, act, exc = collections.Counter(), collections.Counter(), collections.Counter()
    for r in rs:
        cond.update(k for k, v in (r.get("conditions") or {}).items() if v not in (None, [], False))
        act.update(k for k, v in (r.get("actions") or {}).items() if v not in (None, [], False))
        exc.update(k for k, v in (r.get("exceptions") or {}).items() if v not in (None, [], False))
    print(f"rules: {len(rs)}  enabled: {sum(1 for r in rs if r.get('isEnabled'))}  "
          f"readOnly: {sum(1 for r in rs if r.get('isReadOnly'))}  hasError: {sum(1 for r in rs if r.get('hasError'))}")
    print("conditions used:", dict(cond))
    print("actions used:   ", dict(act))
    print("exceptions used:", dict(exc))
    tag_rules = [r for r in rs if (r.get("actions") or {}).get("assignCategories")]
    print("rules that assign categories:", len(tag_rules))
    sizes = sorted(sum(len(v) for v in (r.get("conditions") or {}).values() if isinstance(v, list)) for r in rs)
    print("condition value counts per rule (sorted):", sizes)
    print("master categories:", len(cs) if st2 == 200 else f"failed {st2}")


def scratch():
    g, _ = graph()
    st, before_r = rules(g); st2, before_c = cats(g)
    assert st == 200 and st2 == 200, (st, st2)
    ids_r = sorted(r["id"] for r in before_r); ids_c = sorted(c["id"] for c in before_c)
    assert not any(r["displayName"] == RULE for r in before_r), "leftover spike rule: remove it first"
    print(f"before: {len(ids_r)} rules, {len(ids_c)} categories")
    made_r = made_c = None
    try:
        st, c = g.call("cat create", "POST", "/me/outlook/masterCategories", {"displayName": CAT, "color": "preset3"})
        print("create category:", st); made_c = c and c.get("id")
        st, c2 = g.call("cat rename", "PATCH", f"/me/outlook/masterCategories/{made_c}", {"displayName": CAT + "-x"})
        print("PATCH displayName (rename):", st, gs.short(c2, 200))
        st, c3 = g.call("cat recolor", "PATCH", f"/me/outlook/masterCategories/{made_c}", {"color": "preset5"})
        print("PATCH color:", st)
        # rule is DISABLED, matches an address that cannot exist, and assigns a category name not in the master list
        body = {"displayName": RULE, "sequence": 9999, "isEnabled": False,
                "conditions": {"senderContains": ["omalook-spike-nobody@invalid.invalid"]},
                "actions": {"assignCategories": ["omalook-spike-unlisted"], "stopProcessingRules": False}}
        st, r = g.call("rule create", "POST", "/me/mailFolders/inbox/messageRules", body)
        print("create rule (disabled, unlisted category):", st, gs.short(r, 300)); made_r = r and r.get("id")
        st, r2 = g.call("rule get", "GET", f"/me/mailFolders/inbox/messageRules/{made_r}")
        print("get rule:", st, "actions:", (r2 or {}).get("actions"))
        st, r3 = g.call("rule patch", "PATCH", f"/me/mailFolders/inbox/messageRules/{made_r}",
                        {"conditions": {"senderContains": ["omalook-spike-nobody@invalid.invalid", "second@invalid.invalid"]}})
        print("patch conditions:", st, "->", (r3 or {}).get("conditions"))
        print("etag on rule?", [k for k in (r3 or {}) if "etag" in k.lower() or "changekey" in k.lower()] or "none")
    finally:
        if made_r:
            print("delete rule:", g.call("rule delete", "DELETE", f"/me/mailFolders/inbox/messageRules/{made_r}")[0])
        if made_c:
            print("delete category:", g.call("cat delete", "DELETE", f"/me/outlook/masterCategories/{made_c}")[0])
    st, after_r = rules(g); st2, after_c = cats(g)
    same = sorted(r["id"] for r in after_r) == ids_r and sorted(c["id"] for c in after_c) == ids_c
    print("after:", len(after_r), "rules,", len(after_c), "categories; unchanged from before:", same)
    if not same:
        sys.exit("STATE DIFFERS FROM BEFORE: inspect by hand")


if __name__ == "__main__":
    {"rules": read_only, "scratch": scratch}[sys.argv[1]]()
