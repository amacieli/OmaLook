.pragma library

// Pure helpers for the People view: what a Graph contact shows as, section by section, and what
// the keyboard can act on (mail it, text it, open it, copy it). No QML types, so node can test it.

var MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]

function s(v) { return v === null || v === undefined ? "" : String(v).trim() }
function arr(v) { return Array.isArray(v) ? v : [] }

// What to call the contact: Graph's own name, else given + surname, else the first address.
function displayName(c) {
  c = c || {}
  var d = s(c.displayName)
  if (d !== "") return d
  d = (s(c.givenName) + " " + s(c.surname)).trim()
  if (d !== "") return d
  var e = arr(c.emailAddresses).map(function(x) { return s(x.address) }).filter(Boolean)
  if (e.length) return e[0]
  var co = s(c.companyName)
  return co !== "" ? co : "(no name)"
}

// "street, city, state zip, country" in one line; "" when the address is empty.
function oneLine(a) {
  a = a || {}
  var sz = [s(a.state), s(a.postalCode)].filter(Boolean).join(" ")
  return [s(a.street).replace(/\s*\n\s*/g, ", "), s(a.city), sz, s(a.countryOrRegion)].filter(Boolean).join(", ")
}

// The same address on several lines (for the detail pane).
function manyLines(a) {
  a = a || {}
  var sz = [s(a.city), [s(a.state), s(a.postalCode)].filter(Boolean).join(" ")].filter(Boolean).join(", ")
  return [s(a.street), sz, s(a.countryOrRegion)].filter(Boolean).join("\n")
}

// Graph stores a birthday as that day at 11:59 UTC; a year of 1604 means "no year given" (Outlook's convention).
function birthdayText(iso) {
  var m = /^(\d{4})-(\d{2})-(\d{2})/.exec(s(iso))
  if (!m) return ""
  var mon = MONTHS[parseInt(m[2], 10) - 1], day = parseInt(m[3], 10)
  return m[1] === "1604" || m[1] === "0001" ? day + " " + mon : day + " " + mon + " " + m[1]
}

var EXT_IDS = { fax: "string 0x3a23", pager: "string 0x3a21", car: "string 0x3a1e", companyMain: "string 0x3a57",
                assistantPhone: "string 0x3a2e", homePage: "string 0x3a50", anniversary: "systemtime 0x3a41", gender: "integer 0x3a4d" }
// The MAPI properties the backend filled in (only after the editor has opened the contact).
function ext(c, name) {
  var want = EXT_IDS[name], found = ""
  arr(c && c.singleValueExtendedProperties).forEach(function(p) { if (s(p.id).toLowerCase() === want) found = s(p.value) })
  return found
}

function _row(label, value, kind, extra) {
  var r = { label: label, value: value, kind: kind || "text" }
  if (extra) for (var k in extra) r[k] = extra[k]
  return r
}

// Sections of the detail pane: [{ title, rows: [{label, value, kind, ...}] }]; empty ones are left out.
function sections(c) {
  c = c || {}
  var out = []
  function add(title, rows) { rows = rows.filter(function(r) { return r.value !== "" }); if (rows.length) out.push({ title: title, rows: rows }) }

  add("Name", [
    _row("Title", s(c.title)), _row("First", s(c.givenName)), _row("Middle", s(c.middleName)), _row("Last", s(c.surname)),
    _row("Suffix", s(c.generation)), _row("Nickname", s(c.nickName)), _row("File as", s(c.fileAs)),
    _row("Phonetic first", s(c.yomiGivenName)), _row("Phonetic last", s(c.yomiSurname))
  ])
  add("Email", arr(c.emailAddresses).map(function(e, i) {
    var a = s(e.address), n = s(e.name)
    return _row("Email " + (i + 1), n !== "" && n !== a ? n + " <" + a + ">" : a, "email", { address: a })
  }))
  var phones = []
  arr(c.businessPhones).forEach(function(p) { phones.push(_row("Business", s(p), "phone", { number: s(p) })) })
  arr(c.homePhones).forEach(function(p) { phones.push(_row("Home", s(p), "phone", { number: s(p) })) })
  if (s(c.mobilePhone) !== "") phones.push(_row("Mobile", s(c.mobilePhone), "phone", { number: s(c.mobilePhone) }))
  ;[["Fax", "fax"], ["Pager", "pager"], ["Car", "car"], ["Assistant", "assistantPhone"], ["Company main", "companyMain"]].forEach(function(p) {
    var v = ext(c, p[1]); if (v !== "") phones.push(_row(p[0], v, "phone", { number: v }))
  })
  add("Phone", phones)
  add("Chat", arr(c.imAddresses).map(function(x) { return _row("IM", s(x)) }))
  add("Address", [["Home", c.homeAddress], ["Business", c.businessAddress], ["Other", c.otherAddress]].map(function(p) {
    var t = manyLines(p[1])
    return _row(p[0], t, "address", { query: oneLine(p[1]) })
  }))
  add("Work", [
    _row("Company", s(c.companyName)), _row("Department", s(c.department)), _row("Job title", s(c.jobTitle)),
    _row("Office", s(c.officeLocation)), _row("Profession", s(c.profession)), _row("Manager", s(c.manager)),
    _row("Assistant", s(c.assistantName)), _row("Phonetic company", s(c.yomiCompanyName))
  ])
  add("Personal", [
    _row("Birthday", birthdayText(c.birthday)), _row("Anniversary", birthdayText(ext(c, "anniversary"))),
    _row("Gender", ext(c, "gender") === "1" ? "female" : (ext(c, "gender") === "2" ? "male" : "")), _row("Spouse", s(c.spouseName)),
    _row("Children", arr(c.children).map(s).filter(Boolean).join(", "))
  ])
  add("Web", [_row("Home page", s(c.businessHomePage), "url", { url: s(c.businessHomePage) }),
              _row("Personal page", ext(c, "homePage"), "url", { url: ext(c, "homePage") })])
  add("Categories", [_row("Categories", arr(c.categories).map(s).filter(Boolean).join(", "))])
  add("Notes", [_row("Notes", s(c.personalNotes), "notes")])
  return out
}

// Everything the keys can act on, flattened: [{ kind: email|phone|address|url, label, value, ... }]
function targets(c, kinds) {
  var all = []
  sections(c).forEach(function(sec) {
    sec.rows.forEach(function(r) {
      if (r.kind === "email") all.push({ kind: "email", label: r.label, value: r.address, shown: r.value })
      else if (r.kind === "phone") all.push({ kind: "phone", label: r.label, value: r.number, shown: r.value })
      else if (r.kind === "address") all.push({ kind: "address", label: r.label, value: r.query, shown: r.query })
      else if (r.kind === "url") all.push({ kind: "url", label: r.label, value: r.url, shown: r.url })
    })
  })
  return kinds ? all.filter(function(t) { return kinds.indexOf(t.kind) >= 0 }) : all
}

// A web address the browser can open: bare "example.com" gets https.
function openableUrl(u) {
  u = s(u)
  if (u === "") return ""
  return /^[a-z][a-z0-9+.-]*:/i.test(u) ? u : "https://" + u
}

// A map search URL for an address line.
function mapUrl(line) { return "https://www.openstreetmap.org/search?query=" + encodeURIComponent(line) }

// The contact as plain text (for copying).
function asText(c) {
  var lines = [displayName(c)]
  sections(c).forEach(function(sec) {
    if (sec.title === "Name") return
    sec.rows.forEach(function(r) { lines.push(r.label + ": " + r.value.replace(/\n/g, ", ")) })
  })
  return lines.join("\n")
}

// A vCard 3.0 for one contact (phase 5 export uses the same text).
function vcard(c) {
  function esc(v) { return s(v).replace(/\\/g, "\\\\").replace(/\n/g, "\\n").replace(/,/g, "\\,").replace(/;/g, "\\;") }
  var l = ["BEGIN:VCARD", "VERSION:3.0"]
  l.push("N:" + [esc(c.surname), esc(c.givenName), esc(c.middleName), esc(c.title), esc(c.generation)].join(";"))
  l.push("FN:" + esc(displayName(c)))
  if (s(c.nickName)) l.push("NICKNAME:" + esc(c.nickName))
  if (s(c.companyName) || s(c.department)) l.push("ORG:" + esc(c.companyName) + (s(c.department) ? ";" + esc(c.department) : ""))
  if (s(c.jobTitle)) l.push("TITLE:" + esc(c.jobTitle))
  arr(c.emailAddresses).forEach(function(e) { if (s(e.address)) l.push("EMAIL;TYPE=INTERNET:" + esc(e.address)) })
  arr(c.businessPhones).forEach(function(p) { l.push("TEL;TYPE=WORK,VOICE:" + esc(p)) })
  arr(c.homePhones).forEach(function(p) { l.push("TEL;TYPE=HOME,VOICE:" + esc(p)) })
  if (s(c.mobilePhone)) l.push("TEL;TYPE=CELL:" + esc(c.mobilePhone))
  ;[["HOME", c.homeAddress], ["WORK", c.businessAddress], ["OTHER", c.otherAddress]].forEach(function(p) {
    var a = p[1] || {}
    if (oneLine(a) !== "") l.push("ADR;TYPE=" + p[0] + ":;;" + [esc(a.street), esc(a.city), esc(a.state), esc(a.postalCode), esc(a.countryOrRegion)].join(";"))
  })
  if (s(c.businessHomePage)) l.push("URL:" + esc(c.businessHomePage))
  var b = /^(\d{4})-(\d{2})-(\d{2})/.exec(s(c.birthday))
  if (b) l.push("BDAY:" + (b[1] === "1604" ? "--" + b[2] + b[3] : b[1] + "-" + b[2] + "-" + b[3]))
  if (s(c.personalNotes)) l.push("NOTE:" + esc(c.personalNotes))
  if (arr(c.categories).length) l.push("CATEGORIES:" + arr(c.categories).map(esc).join(","))
  l.push("END:VCARD")
  return l.join("\r\n")
}
