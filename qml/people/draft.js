.pragma library

// The contact editor's model (docs/PLAN-contacts.md section 5): the fields the editor can show, how a
// Graph contact fills them, and how what was typed becomes a save request. Pure functions (node tests).
// The server enforces the same limits: 3 email addresses, 2 business and 2 home phones, one mobile.

// kind: text | notes | date | list (comma separated)
// show: true = in the editor from the start (user decision 2026-11-11: Name, Email, Phone, Company/Title, Notes)
// block: fields that are added together (an address is five rows)
var DEFS = [
  { id: "givenName",  label: "First name",   kind: "text", show: true,  key: "givenName" },
  { id: "surname",    label: "Last name",    kind: "text", show: true,  key: "surname" },
  { id: "middleName", label: "Middle name",  kind: "text", key: "middleName" },
  { id: "title",      label: "Title",        kind: "text", key: "title", hint: "Mr, Dr, Prof" },
  { id: "generation", label: "Suffix",       kind: "text", key: "generation", hint: "Jr, III" },
  { id: "nickName",   label: "Nickname",     kind: "text", key: "nickName" },
  { id: "displayName", label: "Shown as",    kind: "text", key: "displayName", hint: "leave empty to build it from the name" },
  { id: "fileAs",     label: "File as",      kind: "text", key: "fileAs" },
  { id: "yomiGivenName", label: "Phonetic first name", kind: "text", key: "yomiGivenName" },
  { id: "yomiSurname",   label: "Phonetic last name",  kind: "text", key: "yomiSurname" },

  { id: "email0", label: "Email",   kind: "email", show: true, slot: 0 },
  { id: "email1", label: "Email 2", kind: "email", slot: 1 },
  { id: "email2", label: "Email 3", kind: "email", slot: 2 },
  { id: "email0name", label: "Email 1 name", kind: "emailname", slot: 0 },
  { id: "email1name", label: "Email 2 name", kind: "emailname", slot: 1 },
  { id: "email2name", label: "Email 3 name", kind: "emailname", slot: 2 },

  { id: "mobilePhone", label: "Mobile",         kind: "text", show: true, key: "mobilePhone" },
  { id: "business0",   label: "Business phone", kind: "phone", list: "businessPhones", slot: 0 },
  { id: "business1",   label: "Business phone 2", kind: "phone", list: "businessPhones", slot: 1 },
  { id: "home0",       label: "Home phone",     kind: "phone", list: "homePhones", slot: 0 },
  { id: "home1",       label: "Home phone 2",   kind: "phone", list: "homePhones", slot: 1 },
  { id: "imAddresses", label: "Chat (IM)",      kind: "list", key: "imAddresses", hint: "comma separated" },

  { id: "companyName", label: "Company",   kind: "text", show: true, key: "companyName" },
  { id: "jobTitle",    label: "Job title", kind: "text", show: true, key: "jobTitle" },
  { id: "department",  label: "Department", kind: "text", key: "department" },
  { id: "officeLocation", label: "Office", kind: "text", key: "officeLocation" },
  { id: "profession",  label: "Profession", kind: "text", key: "profession" },
  { id: "manager",     label: "Manager",    kind: "text", key: "manager" },
  { id: "assistantName", label: "Assistant", kind: "text", key: "assistantName" },
  { id: "yomiCompanyName", label: "Phonetic company", kind: "text", key: "yomiCompanyName" },

  { id: "businessHomePage", label: "Web page", kind: "text", key: "businessHomePage" },
  { id: "birthday",   label: "Birthday",  kind: "date", key: "birthday", hint: "2000-05-17, or 05-17 without a year" },
  { id: "spouseName", label: "Spouse / partner", kind: "text", key: "spouseName" },
  { id: "children",   label: "Children",  kind: "list", key: "children", hint: "comma separated" },
  { id: "categories", label: "Tags",      kind: "list", key: "categories", hint: "comma separated" },
  { id: "personalNotes", label: "Notes",  kind: "notes", show: true, key: "personalNotes" }
]

// MAPI properties with no field of their own (each checked to round-trip). The editor reads them from
// raw.singleValueExtendedProperties, which the backend fills when the editor opens (GET /contacts/get?live=1).
var EXT_IDS = { fax: "string 0x3a23", pager: "string 0x3a21", car: "string 0x3a1e", companyMain: "string 0x3a57",
                assistantPhone: "string 0x3a2e", homePage: "string 0x3a50", anniversary: "systemtime 0x3a41", gender: "integer 0x3a4d" }
DEFS.push({ id: "x_fax", label: "Fax", kind: "ext", ext: "fax" })
DEFS.push({ id: "x_pager", label: "Pager", kind: "ext", ext: "pager" })
DEFS.push({ id: "x_car", label: "Car phone", kind: "ext", ext: "car" })
DEFS.push({ id: "x_assistantPhone", label: "Assistant phone", kind: "ext", ext: "assistantPhone" })
DEFS.push({ id: "x_companyMain", label: "Company main phone", kind: "ext", ext: "companyMain" })
DEFS.push({ id: "x_homePage", label: "Personal web page", kind: "ext", ext: "homePage" })
DEFS.push({ id: "x_anniversary", label: "Anniversary", kind: "ext", ext: "anniversary", hint: "2010-06-12 (a date cannot be cleared once set)" })
DEFS.push({ id: "x_gender", label: "Gender", kind: "ext", ext: "gender", hint: "female, male or unspecified" })

var ADDRESS_PARTS = [["street", "Street"], ["city", "City"], ["state", "State / region"], ["postalCode", "Postal code"], ["countryOrRegion", "Country"]]
;[["homeAddress", "Home"], ["businessAddress", "Business"], ["otherAddress", "Other"]].forEach(function(a) {
  ADDRESS_PARTS.forEach(function(p, i) {
    DEFS.push({ id: a[0] + "." + p[0], label: a[1] + " " + p[1].toLowerCase(), kind: "address", addr: a[0], part: p[0], block: a[0], first: i === 0, blockLabel: a[1] + " address" })
  })
})

var BY_ID = {}
DEFS.forEach(function(d) { BY_ID[d.id] = d })

function def(id) { return BY_ID[id] || null }
function s(v) { return v === null || v === undefined ? "" : String(v) }

function defaultIds() { return DEFS.filter(function(d) { return d.show === true }).map(function(d) { return d.id }) }

// What a field shows for a contact (the contact as the provider returned it).
function valueOf(raw, d) {
  raw = raw || {}
  switch (d.kind) {
    case "email": { var e = (raw.emailAddresses || [])[d.slot]; return e ? s(e.address) : "" }
    case "emailname": {
      var e2 = (raw.emailAddresses || [])[d.slot]
      if (!e2) return ""
      return s(e2.name) === s(e2.address) ? "" : s(e2.name)
    }
    case "phone": return s((raw[d.list] || [])[d.slot])
    case "list": return (raw[d.key] || []).map(s).join(", ")
    case "address": return s((raw[d.addr] || {})[d.part])
    case "ext": {
      var want = EXT_IDS[d.ext], found = ""
      ;(raw.singleValueExtendedProperties || []).forEach(function(p) { if (s(p.id).toLowerCase() === want) found = s(p.value) })
      if (d.ext === "gender") return found === "1" ? "female" : (found === "2" ? "male" : "")
      if (d.ext === "anniversary") return found.slice(0, 10)
      return found
    }
    case "date": {
      var m = /^(\d{4})-(\d{2})-(\d{2})/.exec(s(raw.birthday))
      return m ? (m[1] === "1604" || m[1] === "0001" ? m[2] + "-" + m[3] : m[1] + "-" + m[2] + "-" + m[3]) : ""
    }
    default: return s(raw[d.key])
  }
}

function valuesOf(raw) {
  var v = {}
  DEFS.forEach(function(d) { v[d.id] = valueOf(raw, d) })
  return v
}

// The ids to show: the defaults, and every field that holds something (a whole address when any part does).
function shownIds(values, extra) {
  var out = {}
  defaultIds().forEach(function(i) { out[i] = true })
  DEFS.forEach(function(d) { if (s(values[d.id]).trim() !== "") { out[d.id] = true } })
  DEFS.forEach(function(d) { if (d.block && out[d.id]) DEFS.forEach(function(o) { if (o.block === d.block) out[o.id] = true }) })
  ;(extra || []).forEach(function(i) { out[i] = true })
  return DEFS.filter(function(d) { return out[d.id] }).map(function(d) { return d.id })
}

// What Ctrl-K may add: every field not shown yet, an address as one entry.
function pickable(shown) {
  var have = {}
  shown.forEach(function(i) { have[i] = true })
  var out = [], blocks = {}
  DEFS.forEach(function(d) {
    if (have[d.id]) return
    if (d.block) {
      if (blocks[d.block]) return
      blocks[d.block] = true
      out.push({ id: d.id, label: d.blockLabel, add: DEFS.filter(function(o) { return o.block === d.block }).map(function(o) { return o.id }) })
    } else out.push({ id: d.id, label: d.label, add: [d.id] })
  })
  return out
}

// "05-17" or "2000-05-17" -> "2000-05-17" / "1604-05-17"; "" -> ""; anything else -> null (not a date).
function parseBirthday(text) {
  var t = s(text).trim()
  if (t === "") return ""
  var m = /^(\d{4})-(\d{1,2})-(\d{1,2})$/.exec(t)
  var y, mo, d
  if (m) { y = parseInt(m[1], 10); mo = parseInt(m[2], 10); d = parseInt(m[3], 10) }
  else {
    m = /^-{0,2}(\d{1,2})-(\d{1,2})$/.exec(t)
    if (!m) return null
    y = 1604; mo = parseInt(m[1], 10); d = parseInt(m[2], 10)
  }
  var dim = [31, (y % 4 === 0 && (y % 100 !== 0 || y % 400 === 0)) ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
  if (mo < 1 || mo > 12 || d < 1 || d > dim[mo - 1]) return null
  var p = function(n) { return (n < 10 ? "0" : "") + n }
  return String(y).padStart(4, "0") + "-" + p(mo) + "-" + p(d)
}

function splitList(text) { return s(text).split(/[,;\n]/).map(function(x) { return x.trim() }).filter(Boolean) }

// First problem found, or "" (the server checks the same).
function validate(values) {
  for (var i = 0; i < 3; i++) {
    var a = s(values["email" + i]).trim()
    if (a !== "" && (!/^[^\s@]+@[^\s@]+$/.test(a))) return "'" + a + "' is not an email address"
    if (a === "" && s(values["email" + i + "name"]).trim() !== "") return "Email " + (i + 1) + " has a name but no address"
  }
  if (s(values.x_anniversary).trim() !== "" && parseBirthday(values.x_anniversary) === null) return "Anniversary: use 2010-06-12"
  if (["", "female", "male", "unspecified"].indexOf(s(values.x_gender).trim().toLowerCase()) < 0) return "Gender: female, male or unspecified"
  if (parseBirthday(values.birthday) === null) return "Birthday: use 2000-05-17, or 05-17 without a year"
  return ""
}

// The fields of a save: only the groups where something changed against `initial` (the values the editor
// opened with); the server drops what is the same again. For a new contact `initial` is {}.
function buildFields(values, initial) {
  initial = initial || {}
  var changed = {}
  DEFS.forEach(function(d) { if (s(values[d.id]).trim() !== s(initial[d.id]).trim()) changed[d.id] = true })
  var f = {}
  function touched(pred) { return DEFS.some(function(d) { return changed[d.id] && pred(d) }) }

  DEFS.forEach(function(d) {
    if (!changed[d.id]) return
    if (d.kind === "text" || d.kind === "notes") f[d.key] = s(values[d.id]).replace(/^\s+|\s+$/g, "")
    else if (d.kind === "list") f[d.key] = splitList(values[d.id])
    else if (d.kind === "date") f.birthday = parseBirthday(values[d.id]) || null
  })
  var ext = {}, anyExt = false
  DEFS.forEach(function(d) { if (d.kind === "ext" && changed[d.id]) { ext[d.ext] = d.ext === "gender" ? s(values[d.id]).trim().toLowerCase() : s(values[d.id]).trim(); anyExt = true } })
  if (anyExt) f.extended = ext
  if (touched(function(d) { return d.kind === "email" || d.kind === "emailname" })) {
    var emails = []
    for (var i = 0; i < 3; i++) {
      var a = s(values["email" + i]).trim(), n = s(values["email" + i + "name"]).trim()
      if (a !== "") emails.push(n !== "" ? { name: n, address: a } : { address: a })
    }
    f.emailAddresses = emails
  }
  ;[["businessPhones", "business"], ["homePhones", "home"]].forEach(function(p) {
    if (touched(function(d) { return d.list === p[0] })) {
      f[p[0]] = [0, 1].map(function(i) { return s(values[p[1] + i]).trim() }).filter(Boolean)
    }
  })
  ;["homeAddress", "businessAddress", "otherAddress"].forEach(function(k) {
    if (touched(function(d) { return d.addr === k })) {
      var o = {}
      ADDRESS_PARTS.forEach(function(p) { o[p[0]] = s(values[k + "." + p[0]]).trim() })
      f[k] = o
    }
  })
  return f
}

// A short summary for the status line: "3 fields changed".
function changeCount(values, initial) {
  var n = 0
  DEFS.forEach(function(d) { if (s(values[d.id]).trim() !== s((initial || {})[d.id]).trim()) n++ })
  return n
}
