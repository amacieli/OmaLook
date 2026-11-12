.pragma library

// OL-023: which recipients of a compose are outside the sender's organisation. Pure.
//
// The banner is for Exchange (Microsoft Graph) work or school accounts only. A Graph account
// on a consumer domain (outlook.com, hotmail.com, ...) is a personal Microsoft account and gets
// no banner. Gmail accounts get no banner. "Inside" is the account's own address domain only:
// the app does not hold a permission that lists the tenant's verified domains (that needs
// Domain.Read.All or Organization.Read.All), so other verified domains of the tenant show as
// external. Anything not in `inside` is external, consumer domains included.

var CONSUMER = [
  "outlook.com", "hotmail.com", "live.com", "live.co.uk", "msn.com", "passport.com",
  "gmail.com", "googlemail.com", "yahoo.com", "yahoo.co.uk", "icloud.com", "me.com",
  "mac.com", "aol.com", "proton.me", "protonmail.com", "gmx.com", "gmx.de", "web.de",
  "mail.com", "zoho.com"
]

function domainOf(email) {
  var s = String(email || "").trim().toLowerCase()
  var i = s.lastIndexOf("@")
  return i < 0 ? "" : s.slice(i + 1)
}

function isConsumer(domain) {
  return CONSUMER.indexOf(String(domain || "").toLowerCase()) >= 0
}

// The domains that count as inside for this account, or null when no banner applies.
// `account` is an entry of GET /accounts: { provider, email, ... }.
function bannerDomains(account) {
  if (!account || account.provider !== "graph") return null
  var d = domainOf(account.email)
  if (d === "" || isConsumer(d)) return null
  return [d]
}

// Which recipients are external. `entries` are RecipientField entries: { email, name, valid } for
// addresses and { list, name } for distribution lists. `listMembers(listId)` returns the member
// entries [{ email }] of a list (or [] when unknown). Duplicates (any case) are reported once.
// Returns [{ email, name }] in the order they were typed.
function externalRecipients(entries, listMembers, inside) {
  var out = [], seen = {}
  if (!inside) return out
  function consider(email, name) {
    var e = String(email || "").trim()
    if (e === "") return
    var key = e.toLowerCase()
    if (inside.indexOf(domainOf(e)) >= 0 || seen[key]) return
    seen[key] = true
    out.push({ email: e, name: name || "" })
  }
  ;(entries || []).forEach(function(r) {
    if (r.list) {
      var members = listMembers ? listMembers(r.list) : []
      ;(members || []).forEach(function(m) { consider(m.email, m.name) })
    } else if (r.valid !== false) {
      consider(r.email, r.name)
    }
  })
  return out
}

// The one-line banner text. It names every external address; it never offers to stop sending.
function bannerText(inside, externals) {
  var names = externals.map(function(x) { return x.email })
  var shown = names.length > 4 ? names.slice(0, 4).join(", ") + " and " + (names.length - 4) + " more" : names.join(", ")
  return "External recipients (outside " + inside.map(function(d) { return "@" + d }).join(", ") + "): " + shown
}
