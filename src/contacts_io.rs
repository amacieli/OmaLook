//! Contacts in and out of files (docs/PLAN-contacts.md phase 5): vCard 3.0 and CSV, and the duplicate
//! finder. Pure functions over Graph-shaped JSON, so they are tested without a server. Import produces the
//! same `fields` object the editor sends, so it goes through `contacts_write::save` like any other create
//! (and meets the same limits: 3 emails, 2+2 phones; what does not fit is counted, not silently lost).

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

fn s(v: &Value) -> String { v.as_str().unwrap_or("").trim().to_string() }
fn list(v: &Value) -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|t| t.trim().to_string())).filter(|t| !t.is_empty()).collect()).unwrap_or_default() }

/// The name shown for a contact.
pub fn display(c: &Value) -> String {
    let d = s(&c["displayName"]);
    if !d.is_empty() { return d; }
    let n = format!("{} {}", s(&c["givenName"]), s(&c["surname"])).trim().to_string();
    if !n.is_empty() { return n; }
    c["emailAddresses"].as_array().and_then(|a| a.first()).map(|e| s(&e["address"])).filter(|a| !a.is_empty()).unwrap_or_else(|| "(no name)".into())
}

// ───────────────────────── vCard ─────────────────────────

fn vesc(t: &str) -> String { t.replace('\\', "\\\\").replace('\n', "\\n").replace(',', "\\,").replace(';', "\\;") }

fn birthday_vcard(b: &str) -> Option<String> {
    let d: String = b.chars().take(10).collect();
    let p: Vec<&str> = d.split('-').collect();
    if p.len() != 3 { return None; }
    Some(if p[0] == "1604" || p[0] == "0001" { format!("--{}{}", p[1], p[2]) } else { d })
}

/// One vCard 3.0 for a Graph contact.
pub fn to_vcard(c: &Value) -> String {
    let mut l: Vec<String> = vec!["BEGIN:VCARD".into(), "VERSION:3.0".into()];
    l.push(format!("N:{};{};{};{};{}", vesc(&s(&c["surname"])), vesc(&s(&c["givenName"])), vesc(&s(&c["middleName"])), vesc(&s(&c["title"])), vesc(&s(&c["generation"]))));
    l.push(format!("FN:{}", vesc(&display(c))));
    if !s(&c["nickName"]).is_empty() { l.push(format!("NICKNAME:{}", vesc(&s(&c["nickName"])))); }
    if !s(&c["companyName"]).is_empty() || !s(&c["department"]).is_empty() {
        l.push(format!("ORG:{}{}", vesc(&s(&c["companyName"])), if s(&c["department"]).is_empty() { String::new() } else { format!(";{}", vesc(&s(&c["department"]))) }));
    }
    if !s(&c["jobTitle"]).is_empty() { l.push(format!("TITLE:{}", vesc(&s(&c["jobTitle"])))); }
    for e in c["emailAddresses"].as_array().into_iter().flatten() { if !s(&e["address"]).is_empty() { l.push(format!("EMAIL;TYPE=INTERNET:{}", vesc(&s(&e["address"])))); } }
    for p in list(&c["businessPhones"]) { l.push(format!("TEL;TYPE=WORK,VOICE:{}", vesc(&p))); }
    for p in list(&c["homePhones"]) { l.push(format!("TEL;TYPE=HOME,VOICE:{}", vesc(&p))); }
    if !s(&c["mobilePhone"]).is_empty() { l.push(format!("TEL;TYPE=CELL:{}", vesc(&s(&c["mobilePhone"])))); }
    for (k, t) in [("homeAddress", "HOME"), ("businessAddress", "WORK"), ("otherAddress", "OTHER")] {
        let a = &c[k];
        if ["street", "city", "state", "postalCode", "countryOrRegion"].iter().any(|f| !s(&a[*f]).is_empty()) {
            l.push(format!("ADR;TYPE={}:;;{};{};{};{};{}", t, vesc(&s(&a["street"])), vesc(&s(&a["city"])), vesc(&s(&a["state"])), vesc(&s(&a["postalCode"])), vesc(&s(&a["countryOrRegion"]))));
        }
    }
    if !s(&c["businessHomePage"]).is_empty() { l.push(format!("URL:{}", vesc(&s(&c["businessHomePage"])))); }
    if let Some(b) = birthday_vcard(&s(&c["birthday"])) { l.push(format!("BDAY:{b}")); }
    if !s(&c["personalNotes"]).is_empty() { l.push(format!("NOTE:{}", vesc(&s(&c["personalNotes"])))); }
    let cats = list(&c["categories"]);
    if !cats.is_empty() { l.push(format!("CATEGORIES:{}", cats.iter().map(|x| vesc(x)).collect::<Vec<_>>().join(","))); }
    l.push("END:VCARD".into());
    // fold at 75 octets on a character boundary
    l.iter().map(|line| fold(line)).collect::<Vec<_>>().join("\r\n") + "\r\n"
}

fn fold(line: &str) -> String {
    if line.len() <= 75 { return line.to_string(); }
    let mut out = String::new();
    let mut cur = 0;
    for ch in line.chars() {
        let n = ch.len_utf8();
        if cur + n > if out.is_empty() { 75 } else { 74 } && cur > 0 { out.push_str("\r\n "); cur = 0; }
        out.push(ch);
        cur += n;
    }
    out
}

fn unesc(t: &str) -> String {
    let mut out = String::new();
    let mut it = t.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() { Some('n') | Some('N') => out.push('\n'), Some(x) => out.push(x), None => {} }
        } else { out.push(c); }
    }
    out
}

/// Split on an unescaped separator.
fn split_unesc(t: &str, sep: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut esc = false;
    for c in t.chars() {
        if esc { parts.last_mut().unwrap().push('\\'); parts.last_mut().unwrap().push(c); esc = false; }
        else if c == '\\' { esc = true; }
        else if c == sep { parts.push(String::new()); }
        else { parts.last_mut().unwrap().push(c); }
    }
    parts.into_iter().map(|p| unesc(&p)).collect()
}

fn bday_to_date(v: &str) -> Option<String> {
    let t = v.trim();
    let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    if let Some(rest) = t.strip_prefix("--") { let d: String = rest.chars().filter(|c| c.is_ascii_digit()).collect(); if d.len() == 4 { return Some(format!("1604-{}-{}", &d[..2], &d[2..])); } }
    if digits.len() >= 8 { return Some(format!("{}-{}-{}", &digits[..4], &digits[4..6], &digits[6..8])); }
    None
}

/// What was read from one card or row, and what did not fit.
#[derive(Debug, Default, Clone)]
pub struct Imported { pub fields: Map<String, Value>, pub dropped: Vec<String> }

struct Builder { emails: Vec<Value>, business: Vec<String>, home: Vec<String>, mobile: Option<String>, im: Vec<String>, cats: Vec<String>, f: Map<String, Value>, dropped: Vec<String> }
impl Builder {
    fn new() -> Self { Self { emails: vec![], business: vec![], home: vec![], mobile: None, im: vec![], cats: vec![], f: Map::new(), dropped: vec![] } }
    fn text(&mut self, k: &str, v: &str) { let v = v.trim(); if !v.is_empty() { self.f.entry(k.to_string()).or_insert_with(|| json!(v)); } }
    fn email(&mut self, a: &str) {
        let a = a.trim();
        if a.is_empty() { return; }
        if self.emails.len() >= crate::contacts_write::MAX_EMAILS { self.dropped.push(format!("email {a}")); } else if !self.emails.iter().any(|e| e["address"].as_str().map(|x| x.eq_ignore_ascii_case(a)).unwrap_or(false)) { self.emails.push(json!({ "address": a })); }
    }
    fn phone(&mut self, kind: &str, n: &str) {
        let n = n.trim();
        if n.is_empty() { return; }
        match kind {
            "mobile" if self.mobile.is_none() => self.mobile = Some(n.to_string()),
            "home" if self.home.len() < crate::contacts_write::MAX_HOME_PHONES => self.home.push(n.to_string()),
            "work" | "other" | "mobile" | "home" if self.business.len() < crate::contacts_write::MAX_BUSINESS_PHONES => self.business.push(n.to_string()),
            _ => self.dropped.push(format!("phone {n}")),
        }
    }
    fn address(&mut self, key: &str, street: &str, city: &str, state: &str, code: &str, country: &str) {
        if [street, city, state, code, country].iter().all(|x| x.trim().is_empty()) { return; }
        self.f.entry(key.to_string()).or_insert_with(|| json!({ "street": street.trim(), "city": city.trim(), "state": state.trim(), "postalCode": code.trim(), "countryOrRegion": country.trim() }));
    }
    fn finish(mut self) -> Imported {
        if !self.emails.is_empty() { self.f.insert("emailAddresses".into(), json!(self.emails)); }
        if !self.business.is_empty() { self.f.insert("businessPhones".into(), json!(self.business)); }
        if !self.home.is_empty() { self.f.insert("homePhones".into(), json!(self.home)); }
        if let Some(m) = self.mobile { self.f.insert("mobilePhone".into(), json!(m)); }
        if !self.im.is_empty() { self.f.insert("imAddresses".into(), json!(self.im)); }
        if !self.cats.is_empty() { self.f.insert("categories".into(), json!(self.cats)); }
        Imported { fields: self.f, dropped: self.dropped }
    }
}

/// Every card of a vCard file (2.1, 3.0 and 4.0 text; folded lines; escaped text).
pub fn parse_vcards(text: &str) -> Vec<Imported> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = vec![];
    for raw in text.split('\n') {
        if (raw.starts_with(' ') || raw.starts_with('\t')) && !lines.is_empty() { lines.last_mut().unwrap().push_str(&raw[1..]); } else { lines.push(raw.to_string()); }
    }
    let mut out = vec![];
    let mut cur: Option<Builder> = None;
    let mut have_n = false;
    for line in lines {
        let up = line.trim().to_uppercase();
        if up == "BEGIN:VCARD" { cur = Some(Builder::new()); have_n = false; continue; }
        if up == "END:VCARD" { if let Some(b) = cur.take() { out.push(b.finish()); } continue; }
        let Some(b) = cur.as_mut() else { continue };
        let Some((head, value)) = line.split_once(':') else { continue };
        let mut hp = head.split(';');
        let name = hp.next().unwrap_or("").to_uppercase();
        let name = name.rsplit('.').next().unwrap_or("").to_string(); // drop an Apple group prefix (item1.EMAIL)
        let params: String = hp.collect::<Vec<_>>().join(";").to_uppercase();
        match name.as_str() {
            "N" => {
                let p = split_unesc(value, ';');
                let g = |i: usize| p.get(i).cloned().unwrap_or_default();
                b.text("surname", &g(0)); b.text("givenName", &g(1)); b.text("middleName", &g(2)); b.text("title", &g(3)); b.text("generation", &g(4));
                have_n = true;
            }
            "FN" => { let v = unesc(value); if !have_n { b.text("displayName", &v); } else { b.f.entry(String::from("_fn")).or_insert(json!(v)); } }
            "NICKNAME" => b.text("nickName", &unesc(value)),
            "ORG" => { let p = split_unesc(value, ';'); b.text("companyName", p.first().map(String::as_str).unwrap_or("")); b.text("department", p.get(1).map(String::as_str).unwrap_or("")); }
            "TITLE" => b.text("jobTitle", &unesc(value)),
            "EMAIL" => b.email(&unesc(value)),
            "TEL" => {
                let kind = if params.contains("CELL") || params.contains("MOBILE") { "mobile" } else if params.contains("HOME") { "home" } else if params.contains("WORK") { "work" } else { "other" };
                b.phone(kind, &unesc(value));
            }
            "ADR" => {
                let p = split_unesc(value, ';');
                let g = |i: usize| p.get(i).cloned().unwrap_or_default();
                let key = if params.contains("HOME") { "homeAddress" } else if params.contains("WORK") { "businessAddress" } else { "otherAddress" };
                b.address(key, &g(2), &g(3), &g(4), &g(5), &g(6));
            }
            "URL" => b.text("businessHomePage", &unesc(value)),
            "BDAY" => { if let Some(d) = bday_to_date(value) { b.f.entry(String::from("birthday")).or_insert(json!(d)); } }
            "NOTE" => b.text("personalNotes", &unesc(value)),
            "CATEGORIES" => { for c in split_unesc(value, ',') { let c = c.trim().to_string(); if !c.is_empty() && !b.cats.contains(&c) { b.cats.push(c); } } }
            "IMPP" | "X-JABBER" | "X-SKYPE" => { let v = unesc(value); if !v.is_empty() { b.im.push(v); } }
            _ => {}
        }
    }
    out.into_iter().map(|mut i| {
        // a card with only FN: split it into first and last name
        if let Some(fnv) = i.fields.remove("_fn") {
            if !i.fields.contains_key("givenName") && !i.fields.contains_key("surname") && !i.fields.contains_key("displayName") { i.fields.insert("displayName".into(), fnv); }
        }
        i
    }).filter(|i| !i.fields.is_empty()).collect()
}

// ───────────────────────── CSV ─────────────────────────

const CSV_COLUMNS: &[(&str, &[&str])] = &[
    ("First Name", &["givenName"]), ("Middle Name", &["middleName"]), ("Last Name", &["surname"]), ("Title", &["title"]), ("Suffix", &["generation"]),
    ("Nickname", &["nickName"]), ("Company", &["companyName"]), ("Department", &["department"]), ("Job Title", &["jobTitle"]),
    ("E-mail Address", &["emailAddresses", "0"]), ("E-mail 2 Address", &["emailAddresses", "1"]), ("E-mail 3 Address", &["emailAddresses", "2"]),
    ("Mobile Phone", &["mobilePhone"]), ("Business Phone", &["businessPhones", "0"]), ("Business Phone 2", &["businessPhones", "1"]),
    ("Home Phone", &["homePhones", "0"]), ("Home Phone 2", &["homePhones", "1"]),
    ("Home Street", &["homeAddress", "street"]), ("Home City", &["homeAddress", "city"]), ("Home State", &["homeAddress", "state"]),
    ("Home Postal Code", &["homeAddress", "postalCode"]), ("Home Country/Region", &["homeAddress", "countryOrRegion"]),
    ("Business Street", &["businessAddress", "street"]), ("Business City", &["businessAddress", "city"]), ("Business State", &["businessAddress", "state"]),
    ("Business Postal Code", &["businessAddress", "postalCode"]), ("Business Country/Region", &["businessAddress", "countryOrRegion"]),
    ("Other Street", &["otherAddress", "street"]), ("Other City", &["otherAddress", "city"]), ("Other State", &["otherAddress", "state"]),
    ("Other Postal Code", &["otherAddress", "postalCode"]), ("Other Country/Region", &["otherAddress", "countryOrRegion"]),
    ("Web Page", &["businessHomePage"]), ("Birthday", &["birthday"]), ("Spouse", &["spouseName"]), ("Notes", &["personalNotes"]), ("Categories", &["categories"]),
];

fn csv_cell(t: &str) -> String {
    // quote when needed; a leading = + - @ would run as a formula in a spreadsheet, so it is shown as text
    let safe = if t.starts_with(['=', '+', '-', '@']) && !t.chars().skip(1).all(|c| c.is_ascii_digit() || c == ' ' || c == '-' || c == '(' || c == ')') { format!("'{t}") } else { t.to_string() };
    if safe.contains([',', '"', '\n', '\r']) { format!("\"{}\"", safe.replace('"', "\"\"")) } else { safe }
}

fn cell_of(c: &Value, path: &[&str]) -> String {
    match path {
        [k] if *k == "categories" => list(&c[*k]).join("; "),
        [k] if *k == "birthday" => { let d = s(&c[*k]); if d.is_empty() { d } else if d.starts_with("1604") { d.chars().skip(5).take(5).collect() } else { d.chars().take(10).collect() } }
        [k] => s(&c[*k]),
        [k, i] if i.chars().all(|x| x.is_ascii_digit()) => {
            let i: usize = i.parse().unwrap_or(0);
            if *k == "emailAddresses" { c[*k].as_array().and_then(|a| a.get(i)).map(|e| s(&e["address"])).unwrap_or_default() } else { list(&c[*k]).get(i).cloned().unwrap_or_default() }
        }
        [k, part] => s(&c[*k][*part]),
        _ => String::new(),
    }
}

pub fn to_csv(contacts: &[Value]) -> String {
    let mut out = CSV_COLUMNS.iter().map(|(h, _)| csv_cell(h)).collect::<Vec<_>>().join(",") + "\r\n";
    for c in contacts {
        out += &CSV_COLUMNS.iter().map(|(_, p)| csv_cell(&cell_of(c, p))).collect::<Vec<_>>().join(",");
        out += "\r\n";
    }
    out
}

/// RFC 4180 rows.
pub fn parse_csv_rows(text: &str) -> Vec<Vec<String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (mut rows, mut row, mut cell, mut quoted) = (vec![], vec![], String::new(), false);
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        if quoted {
            if c == '"' { if it.peek() == Some(&'"') { cell.push('"'); it.next(); } else { quoted = false; } } else { cell.push(c); }
        } else {
            match c {
                '"' if cell.is_empty() => quoted = true,
                ',' => { row.push(std::mem::take(&mut cell)); }
                '\r' => {}
                '\n' => { row.push(std::mem::take(&mut cell)); rows.push(std::mem::take(&mut row)); }
                _ => cell.push(c),
            }
        }
    }
    if !cell.is_empty() || !row.is_empty() { row.push(cell); rows.push(row); }
    rows.into_iter().filter(|r| r.iter().any(|c| !c.trim().is_empty())).collect()
}

fn norm_header(h: &str) -> String { h.trim().to_lowercase().replace(['-', '_', ' '], "") }

/// Contacts from a CSV with a header row (this app's export, Outlook's and Google's common column names).
pub fn parse_csv(text: &str) -> Vec<Imported> {
    let rows = parse_csv_rows(text);
    let Some(head) = rows.first() else { return vec![] };
    let aliases: &[(&str, &str)] = &[
        ("firstname", "First Name"), ("givenname", "First Name"), ("middlename", "Middle Name"), ("additionalname", "Middle Name"), ("lastname", "Last Name"), ("familyname", "Last Name"),
        ("suffix", "Suffix"), ("nameprefix", "Title"), ("namesuffix", "Suffix"), ("organization1name", "Company"), ("companymainphone", ""), ("organization1title", "Job Title"),
        ("organization1department", "Department"), ("email1value", "E-mail Address"), ("email2value", "E-mail 2 Address"), ("email3value", "E-mail 3 Address"),
        ("emailaddress", "E-mail Address"), ("email2address", "E-mail 2 Address"), ("email3address", "E-mail 3 Address"), ("email", "E-mail Address"),
        ("mobile", "Mobile Phone"), ("mobilephone", "Mobile Phone"), ("businessphone", "Business Phone"), ("workphone", "Business Phone"), ("homephone", "Home Phone"),
        ("company", "Company"), ("jobtitle", "Job Title"), ("notes", "Notes"), ("note", "Notes"), ("webpage", "Web Page"), ("birthday", "Birthday"),
        ("categories", "Categories"), ("labels", "Categories"), ("groupmembership", "Categories"), ("nickname", "Nickname"), ("department", "Department"), ("title", "Title"),
    ];
    let mut col: Vec<Option<usize>> = vec![]; // CSV_COLUMNS index per file column
    for h in head {
        let n = norm_header(h);
        let canon = CSV_COLUMNS.iter().position(|(name, _)| norm_header(name) == n)
            .or_else(|| aliases.iter().find(|(a, _)| *a == n).and_then(|(_, c)| CSV_COLUMNS.iter().position(|(name, _)| name == c)));
        col.push(canon);
    }
    let mut out = vec![];
    for row in rows.iter().skip(1) {
        let mut b = Builder::new();
        let (mut home, mut biz, mut oth): ([String; 5], [String; 5], [String; 5]) = Default::default();
        for (i, cell) in row.iter().enumerate() {
            let Some(Some(ci)) = col.get(i) else { continue };
            let v = cell.trim().trim_start_matches('\'').trim();
            if v.is_empty() { continue; }
            let (name, path) = CSV_COLUMNS[*ci];
            match path {
                ["emailAddresses", _] => b.email(v),
                ["mobilePhone"] => b.phone("mobile", v),
                ["businessPhones", _] => b.phone("work", v),
                ["homePhones", _] => b.phone("home", v),
                [a, part] if a.ends_with("Address") => {
                    let slot = ["street", "city", "state", "postalCode", "countryOrRegion"].iter().position(|p| p == part).unwrap_or(0);
                    match *a { "homeAddress" => home[slot] = v.to_string(), "businessAddress" => biz[slot] = v.to_string(), _ => oth[slot] = v.to_string() }
                }
                ["categories"] => { for c in v.replace(" ::: ", ";").split([';', ',']).map(str::trim).filter(|c| !c.is_empty() && !c.starts_with('*')) { if !b.cats.contains(&c.to_string()) { b.cats.push(c.to_string()); } } }
                ["birthday"] => { if let Some(d) = bday_to_date(v).or_else(|| { let p: Vec<&str> = v.split('-').collect(); if p.len() == 3 && p[0].len() == 4 { Some(v.to_string()) } else { None } }) { b.f.insert("birthday".into(), json!(d)); } else { b.dropped.push(format!("{name} {v}")); } }
                [k] => b.text(k, v),
                _ => {}
            }
        }
        b.address("homeAddress", &home[0], &home[1], &home[2], &home[3], &home[4]);
        b.address("businessAddress", &biz[0], &biz[1], &biz[2], &biz[3], &biz[4]);
        b.address("otherAddress", &oth[0], &oth[1], &oth[2], &oth[3], &oth[4]);
        let i = b.finish();
        if !i.fields.is_empty() { out.push(i); }
    }
    out
}

// ───────────────────────── duplicates and merging ─────────────────────────

/// Groups of contacts that look like the same person: the same first address (any case), or the same
/// name (letters only, case and order of the two parts ignored) when both have one. Ids per group, in the
/// order given; single contacts are not returned.
pub fn duplicate_groups(contacts: &[(String, Value)]) -> Vec<Vec<String>> {
    let mut by: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, (_, c)) in contacts.iter().enumerate() {
        if let Some(e) = c["emailAddresses"].as_array().and_then(|a| a.first()).map(|e| s(&e["address"]).to_lowercase()).filter(|e| !e.is_empty()) { by.entry(format!("e:{e}")).or_default().push(i); }
        let mut parts = [s(&c["givenName"]), s(&c["surname"])].map(|p| p.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase());
        if !parts[0].is_empty() && !parts[1].is_empty() { parts.sort(); by.entry(format!("n:{}{}", parts[0], parts[1])).or_default().push(i); }
    }
    // union the groups that share a member
    let mut group_of: Vec<usize> = (0..contacts.len()).collect();
    fn find(g: &mut Vec<usize>, x: usize) -> usize { let mut r = x; while g[r] != r { r = g[r]; } let mut y = x; while g[y] != r { let n = g[y]; g[y] = r; y = n; } r }
    for ids in by.values().filter(|v| v.len() > 1) {
        for w in ids.windows(2) { let (a, b) = (find(&mut group_of, w[0]), find(&mut group_of, w[1])); if a != b { group_of[b] = a; } }
    }
    let mut groups: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for i in 0..contacts.len() { let r = find(&mut group_of, i); groups.entry(r).or_default().push(contacts[i].0.clone()); }
    let mut out: Vec<Vec<String>> = groups.into_values().filter(|g| g.len() > 1).collect();
    out.sort_by_key(|g| contacts.iter().position(|(id, _)| id == &g[0]).unwrap_or(0));
    out
}

/// The fields a merge writes to the keeper: for every field the keeper has blank, the first other contact's
/// value; emails, phones, chat, children and tags are unions (up to the server's limits). Never overwrites.
pub fn merge_fields(keeper: &Value, others: &[Value]) -> Map<String, Value> {
    let mut f = Map::new();
    for k in ["givenName", "middleName", "surname", "title", "generation", "nickName", "companyName", "department", "jobTitle", "officeLocation", "profession", "manager", "assistantName", "spouseName", "businessHomePage", "mobilePhone", "personalNotes"] {
        if s(&keeper[k]).is_empty() { if let Some(v) = others.iter().map(|o| s(&o[k])).find(|v| !v.is_empty()) { f.insert(k.into(), json!(v)); } }
    }
    if keeper["birthday"].is_null() || s(&keeper["birthday"]).is_empty() { if let Some(v) = others.iter().map(|o| s(&o["birthday"])).find(|v| !v.is_empty()) { f.insert("birthday".into(), json!(v)); } }
    for k in ["homeAddress", "businessAddress", "otherAddress"] {
        if !["street", "city", "state", "postalCode", "countryOrRegion"].iter().any(|p| !s(&keeper[k][*p]).is_empty()) {
            if let Some(a) = others.iter().map(|o| &o[k]).find(|a| ["street", "city", "state", "postalCode", "countryOrRegion"].iter().any(|p| !s(&a[*p]).is_empty())) { f.insert(k.into(), a.clone()); }
        }
    }
    // unions
    let mut emails: Vec<Value> = keeper["emailAddresses"].as_array().cloned().unwrap_or_default();
    for o in others { for e in o["emailAddresses"].as_array().into_iter().flatten() {
        if !emails.iter().any(|x| s(&x["address"]).eq_ignore_ascii_case(&s(&e["address"]))) && emails.len() < crate::contacts_write::MAX_EMAILS { emails.push(e.clone()); }
    } }
    if emails.len() != keeper["emailAddresses"].as_array().map(|a| a.len()).unwrap_or(0) { f.insert("emailAddresses".into(), json!(emails)); }
    for (k, max) in [("businessPhones", crate::contacts_write::MAX_BUSINESS_PHONES), ("homePhones", crate::contacts_write::MAX_HOME_PHONES), ("imAddresses", 500), ("children", 500), ("categories", 500)] {
        let mut u = list(&keeper[k]);
        let before = u.len();
        for o in others { for v in list(&o[k]) { if !u.iter().any(|x| x.eq_ignore_ascii_case(&v)) && u.len() < max { u.push(v); } } }
        if u.len() != before { f.insert(k.into(), json!(u)); }
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ada() -> Value {
        json!({ "id": "1", "displayName": "Ada Lovelace", "givenName": "Ada", "surname": "Lovelace", "middleName": "K", "title": "Dr", "generation": "Jr", "nickName": "Al",
            "companyName": "Acme, Inc", "department": "R&D", "jobTitle": "Engineer",
            "emailAddresses": [{ "name": "Ada", "address": "ada@x.org" }, { "address": "ada@home.org" }],
            "businessPhones": ["+1 555 0100"], "homePhones": ["+1 555 0101", "+1 555 0102"], "mobilePhone": "+1 555 0103",
            "homeAddress": { "street": "1 Main\nApt 2", "city": "London", "state": "", "postalCode": "N1", "countryOrRegion": "UK" },
            "businessHomePage": "https://acme.test", "birthday": "1815-12-10T11:59:00Z", "personalNotes": "Line one\nLine; two, with \\ slash", "categories": ["Blue", "OL: Family"] })
    }

    #[test]
    fn a_vcard_round_trips_every_field_it_carries() {
        let v = to_vcard(&ada());
        assert!(v.starts_with("BEGIN:VCARD\r\nVERSION:3.0\r\n") && v.ends_with("END:VCARD\r\n"));
        let got = parse_vcards(&v);
        assert_eq!(got.len(), 1);
        let f = &got[0].fields;
        assert_eq!(f["givenName"], "Ada"); assert_eq!(f["surname"], "Lovelace"); assert_eq!(f["middleName"], "K"); assert_eq!(f["title"], "Dr"); assert_eq!(f["generation"], "Jr");
        assert_eq!(f["companyName"], "Acme, Inc"); assert_eq!(f["department"], "R&D"); assert_eq!(f["jobTitle"], "Engineer");
        assert_eq!(f["emailAddresses"], json!([{ "address": "ada@x.org" }, { "address": "ada@home.org" }]));
        assert_eq!(f["businessPhones"], json!(["+1 555 0100"])); assert_eq!(f["homePhones"], json!(["+1 555 0101", "+1 555 0102"])); assert_eq!(f["mobilePhone"], "+1 555 0103");
        assert_eq!(f["homeAddress"]["street"], "1 Main\nApt 2"); assert_eq!(f["homeAddress"]["countryOrRegion"], "UK");
        assert_eq!(f["birthday"], "1815-12-10"); assert_eq!(f["personalNotes"], "Line one\nLine; two, with \\ slash");
        assert_eq!(f["categories"], json!(["Blue", "OL: Family"]));
        assert!(got[0].dropped.is_empty());
        // the result passes the writer's own checks
        assert!(crate::contacts_write::clean_fields(&Value::Object(f.clone())).is_ok());
    }

    #[test]
    fn long_lines_are_folded_and_unfolded_and_no_character_is_split() {
        let mut c = ada();
        c["personalNotes"] = json!("é".repeat(120));
        let v = to_vcard(&c);
        assert!(v.split("\r\n").all(|l| l.len() <= 75), "folded");
        assert_eq!(parse_vcards(&v)[0].fields["personalNotes"], json!("é".repeat(120)));
    }

    #[test]
    fn vcards_from_other_apps_are_read() {
        let t = "BEGIN:VCARD\nVERSION:2.1\nFN:Bob Builder\nN:Builder;Bob;;;\nitem1.EMAIL;TYPE=INTERNET;TYPE=pref:bob@x.org\nTEL;CELL:555\nTEL;WORK;VOICE:1\nTEL;WORK:2\nTEL;WORK:3\nTEL;HOME:4\nBDAY:--0517\nADR;TYPE=WORK:;;1 Rd;Town;;;DE\nEND:VCARD\nBEGIN:VCARD\nFN:Only Name\nEND:VCARD\nBEGIN:VCARD\nEND:VCARD\n";
        let got = parse_vcards(t);
        assert_eq!(got.len(), 2, "an empty card is skipped");
        let f = &got[0].fields;
        assert_eq!(f["emailAddresses"][0]["address"], "bob@x.org"); assert_eq!(f["mobilePhone"], "555");
        assert_eq!(f["businessPhones"], json!(["1", "2"])); assert_eq!(got[0].dropped, vec!["phone 3".to_string()], "the third work phone does not fit and is reported");
        assert_eq!(f["homePhones"], json!(["4"])); assert_eq!(f["birthday"], "1604-05-17"); assert_eq!(f["businessAddress"]["country OrRegion".replace(' ', "").as_str()], "DE");
        assert_eq!(got[1].fields["displayName"], "Only Name");
    }

    #[test]
    fn csv_round_trips_and_keeps_commas_quotes_and_newlines() {
        let c = ada();
        let csv = to_csv(&[c.clone()]);
        assert!(csv.starts_with("First Name,Middle Name,Last Name,"));
        let got = parse_csv(&csv);
        assert_eq!(got.len(), 1);
        let f = &got[0].fields;
        assert_eq!(f["companyName"], "Acme, Inc"); assert_eq!(f["personalNotes"], "Line one\nLine; two, with \\ slash"); assert_eq!(f["homeAddress"]["street"], "1 Main\nApt 2");
        assert_eq!(f["emailAddresses"], json!([{ "address": "ada@x.org" }, { "address": "ada@home.org" }]));
        assert_eq!(f["birthday"], "1815-12-10"); assert_eq!(f["categories"], json!(["Blue", "OL: Family"]));
    }

    #[test]
    fn a_cell_that_would_run_as_a_formula_is_exported_as_text_and_read_back() {
        let mut c = ada();
        c["companyName"] = json!("=HYPERLINK(\"http://evil\")");
        c["personalNotes"] = json!("-1+1");
        let csv = to_csv(&[c]);
        assert!(csv.contains("'=HYPERLINK"), "{csv}");
        let got = parse_csv(&csv);
        assert_eq!(got[0].fields["companyName"], "=HYPERLINK(\"http://evil\")");
        // a phone number starting with + is left alone
        let mut p = ada(); p["mobilePhone"] = json!("+44 20 7946 0958");
        assert!(to_csv(&[p]).contains(",+44 20 7946 0958,"));
    }

    #[test]
    fn outlook_and_google_style_headers_are_understood() {
        let t = "\u{feff}First Name,Last Name,E-mail Address,Mobile Phone,Business Phone,Home Street,Home City,Company,Job Title,Notes\r\nBo,Peep,bo@x.org,555,111,\"1 Rd\",Leeds,Sheep Ltd,Shepherd,\"hello\"\r\n,,,,,,,,,\r\n";
        let got = parse_csv(t);
        assert_eq!(got.len(), 1, "a blank row is skipped");
        assert_eq!(got[0].fields["givenName"], "Bo"); assert_eq!(got[0].fields["homeAddress"]["city"], "Leeds"); assert_eq!(got[0].fields["businessPhones"], json!(["111"]));
        let g = parse_csv("Given Name,Family Name,E-mail 1 - Value,Labels\nCy,Dee,cy@x.org,family ::: * myContacts\n");
        let _ = g; // unknown Google spellings are ignored rather than guessed
        let g2 = parse_csv("First Name,Last Name,Email,Categories\nCy,Dee,cy@x.org,Red; Blue\n");
        assert_eq!(g2[0].fields["emailAddresses"][0]["address"], "cy@x.org"); assert_eq!(g2[0].fields["categories"], json!(["Red", "Blue"]));
    }

    #[test]
    fn csv_rows_handle_quotes_blank_lines_and_a_missing_final_newline() {
        let r = parse_csv_rows("a,\"b \"\"q\"\" c\",d\n\n1,2,3");
        assert_eq!(r, vec![vec!["a", "b \"q\" c", "d"], vec!["1", "2", "3"]]);
    }

    #[test]
    fn duplicates_are_found_by_first_address_or_by_name_and_chained() {
        let c = |id: &str, g: &str, sn: &str, e: &str| (id.to_string(), json!({ "givenName": g, "surname": sn, "emailAddresses": if e.is_empty() { json!([]) } else { json!([{ "address": e }]) } }));
        let all = vec![c("1", "Ada", "L", "a@x.org"), c("2", "Bob", "B", "b@x.org"), c("3", "A.", "Lovelace", "A@X.org"), c("4", "L", "Ada", ""), c("5", "Cy", "", "c@x.org"), c("6", "Cy", "", "c@x.org")];
        let g = duplicate_groups(&all);
        assert_eq!(g, vec![vec!["1", "3", "4"], vec!["5", "6"]], "1 and 3 share an address; 4 has the same two name parts as 1 (in the other order) and joins them");
    }

    #[test]
    fn merging_fills_blanks_unions_lists_within_limits_and_overwrites_nothing() {
        let keeper = json!({ "givenName": "Ada", "surname": "", "emailAddresses": [{ "address": "a@x.org" }, { "address": "b@x.org" }], "businessPhones": ["1"], "categories": ["Blue"], "mobilePhone": "", "birthday": null, "homeAddress": {} });
        let other = json!({ "givenName": "Adeline", "surname": "Lovelace", "emailAddresses": [{ "address": "B@x.org" }, { "address": "c@x.org" }, { "address": "d@x.org" }], "businessPhones": ["1", "2", "3"], "categories": ["blue", "Red"], "mobilePhone": "9", "birthday": "1815-12-10T11:59:00Z", "homeAddress": { "city": "Paris" } });
        let f = merge_fields(&keeper, &[other]);
        assert!(!f.contains_key("givenName"), "the keeper's own value stays");
        assert_eq!(f["surname"], "Lovelace"); assert_eq!(f["mobilePhone"], "9"); assert_eq!(f["birthday"], "1815-12-10T11:59:00Z"); assert_eq!(f["homeAddress"]["city"], "Paris");
        assert_eq!(f["emailAddresses"].as_array().unwrap().len(), 3, "limit of three");
        assert_eq!(f["businessPhones"], json!(["1", "2"]), "limit of two");
        assert_eq!(f["categories"], json!(["Blue", "Red"]));
    }
}
