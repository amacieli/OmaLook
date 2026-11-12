.pragma library

// OL-051: send-as identities in the compose From dropdown. Pure; the backend (`GET /compose/identities`) supplies the
// addresses and the per-account default, `src/identities.rs` repeats the same rules and checks them again at send.
//
//   identities: { accountId: { identities: [{ email, name, primary }], default: "addr" } }

var SEP = "\t"

function _same(a, b) { return String(a || "").trim().toLowerCase() === String(b || "").trim().toLowerCase() }

function _list(identities, account) {
  var e = identities && account ? identities[account.id] : null
  if (e && e.identities && e.identities.length > 0) return e.identities
  return account && account.email ? [{ email: account.email, name: "", primary: true }] : []
}

// Dropdown options: every identity of every signed-in account. The value carries the account and the address.
function options(accounts, identities) {
  var out = []
  ;(accounts || []).filter(function(a) { return a.signed_in && a.email }).forEach(function(a) {
    _list(identities, a).forEach(function(i) {
      out.push({ value: a.id + SEP + i.email, label: i.email + (i.primary ? "" : "  (alias)") })
    })
  })
  return out
}

// The row is shown with more than one account or more than one identity on the current account.
function showRow(accounts, identities, accountId) {
  var signed = (accounts || []).filter(function(a) { return a.signed_in && a.email })
  if (signed.length > 1) return true
  return signed.length === 1 && _list(identities, signed[0]).length > 1
}

function key(accountId, email) { return accountId + SEP + email }
function parse(k) {
  var i = String(k).indexOf(SEP)
  return i < 0 ? { account: String(k), email: "" } : { account: k.substring(0, i), email: k.substring(i + 1) }
}

// The address a message starts with. A reply or reply-all keeps the identity the original was addressed to (any of its
// To / Cc addresses that is one of ours); anything else starts as the account's default.
function autoFrom(identities, account, kind, originalRecipients) {
  if (!account) return ""
  var list = _list(identities, account)
  if (kind === "reply" || kind === "replyAll") {
    var rs = originalRecipients || []
    for (var r = 0; r < rs.length; r++)
      for (var i = 0; i < list.length; i++) if (_same(list[i].email, rs[r])) return list[i].email
  }
  var e = identities && identities[account.id]
  if (e && e.default) for (var d = 0; d < list.length; d++) if (_same(list[d].email, e.default)) return list[d].email
  return account.email
}

// What goes in the message's `from` field: nothing when it is the account's own address.
function override(address, account) {
  if (!account || !address || _same(address, account.email)) return ""
  return address
}

// Every address of the user (all accounts, all identities), lower case: reply-all must not cc the user's own aliases.
function ownAddresses(accounts, identities) {
  var out = []
  ;(accounts || []).forEach(function(a) {
    if (a.email) out.push(a.email)
    _list(identities, a).forEach(function(i) { out.push(i.email) })
  })
  return out
}
