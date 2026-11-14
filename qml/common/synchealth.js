.pragma library

// Sync-thread health -> status-bar warning. Threads come from GET /sync/health:
// [{ name, account, state: "running"|"restarting"|"stopped", ... }].

function problems(threads) {
    return (threads || []).filter(function(t) { return t && t.state && t.state !== "running" })
}

function _label(t, accounts) {
    var a = null
    if (t.account) {
        for (var i = 0; i < (accounts || []).length; i++)
            if (accounts[i].id === t.account) { a = accounts[i]; break }
    }
    var who = a ? (a.display_name || a.name || a.email || "") : ""
    return t.name + (who ? " (" + who + ")" : "")
}

// Short warning for the status bar; "" when nothing is wrong. Stopped wins over restarting.
function warning(threads, accounts) {
    var p = problems(threads)
    if (p.length === 0) return ""
    var stopped = p.filter(function(t) { return t.state === "stopped" })
    var list = stopped.length > 0 ? stopped : p
    var text = (stopped.length > 0 ? "Sync stopped: " : "Sync restarting: ") + _label(list[0], accounts)
    if (list.length > 1) text += " +" + (list.length - 1) + " more"
    return text
}

// Every problem thread, one per line, for a tooltip.
function detail(threads, accounts) {
    return problems(threads).map(function(t) {
        var s = _label(t, accounts) + " \u2014 " + t.state
        if (t.restarts) s += ", " + t.restarts + (t.restarts === 1 ? " restart" : " restarts")
        if (t.last_panic) s += ": " + t.last_panic
        return s
    }).join("\n")
}
