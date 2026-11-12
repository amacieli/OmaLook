.pragma library

// OL-052: bounce (resend a received message to other people, mutt's `b`). Pure; the daemon does the sending
// (src/bounce.rs). Neither Gmail nor Exchange can resend as the original sender, so the dialog says exactly what the
// recipient will see before anything is queued.

var ADDR = /^[^\s@<>,;"()]+@[^\s@<>,;"()]+\.[^\s@<>,;"()]+$/

// "a@x.org, Bob <b@y.org>; c@z.org" -> { to: [{email, name}], bad: [text that is no address] }
function parse(text) {
  var to = [], bad = []
  String(text || "").split(/[,;](?=(?:[^"]*"[^"]*")*[^"]*$)/).forEach(function(tok) {
    var t = tok.trim()
    if (t === "") return
    var m = /^(.*?)<([^<>]+)>\s*$/.exec(t)
    var email = (m ? m[2] : t).trim(), name = m ? m[1].trim().replace(/^"(.*)"$/, "$1") : ""
    if (!ADDR.test(email)) { bad.push(t); return }
    if (!to.some(function(x) { return x.email.toLowerCase() === email.toLowerCase() })) to.push({ email: email, name: name })
  })
  return { to: to, bad: bad }
}

function _plural(n, one, many) { return n === 1 ? one : many }

// What the confirmation dialog says. `info`: { subject, sender (display text of the original From), replyTo (display
// text, "" when the original has none), account (the user's address), delaySecs }.
function notice(info, to) {
  var names = to.map(function(t) { return t.email }).join(", ")
  var lines = []
  lines.push("Bounce \u201c" + (info.subject || "(no subject)") + "\u201d to " + names + "?")
  lines.push("")
  lines.push(_plural(to.length, "They", "They") + " will see it as a message from YOU (" + info.account + "), not from " + (info.sender || "the original sender") +
             ": Gmail and Exchange cannot send as someone else, so this is not a true bounce.")
  lines.push("Subject, text and attachments are unchanged; replies go to " + (info.replyTo || info.sender || "the original sender") + ". The original recipients are not copied.")
  lines.push(info.delaySecs > 0 ? "It goes out in " + info.delaySecs + " s; u takes it back until then." : "It goes out at once (the send delay is 0).")
  return lines.join("\n")
}

// The message /compose/send queues. Nothing is typed: the daemon takes the original's own text and files.
function message(info, to) {
  return {
    kind: "bounce", account_id: info.accountId, in_reply_to: info.messageId,
    to: to.map(function(t) { return { email: t.email, name: t.name || "" } }), cc: [], bcc: [],
    subject: info.subject || "", mode: "system", body: { format: "text", content: "" }, attachments: []
  }
}
