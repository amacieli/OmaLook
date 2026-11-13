pragma Singleton
import QtQuick
import Quickshell

// The one HTTP client for the OmaLook backend (roadmap R1.1, ARCH-T1).
//
// Every request from the UI to the backend goes through request() or requestSync() here, so the
// port override (OMALOOK_PORT) and the per-launch session token (OMALOOK_TOKEN, R1.4) live in one place.
// Never construct an XMLHttpRequest anywhere else in qml/ (`rg XMLHttpRequest qml/` must list only
// this file); a request built elsewhere silently skips the token and can reach the live backend
// from a scratch instance.
QtObject {
  id: root

  // OMALOOK_PORT lets a scratch instance's UI talk to its own backend; the default is the real one.
  readonly property string port: Quickshell.env("OMALOOK_PORT") || "27182"
  readonly property string baseUrl: "http://127.0.0.1:" + port
  // The backend refuses (401) every request without it. Set by the launcher; never logged or shown.
  readonly property string _token: Quickshell.env("OMALOOK_TOKEN") || ""

  // Headers every request carries: the session token, then the caller's own.
  function _prepare(xhr, headers) {
    if (_token !== "") xhr.setRequestHeader("Authorization", "Bearer " + _token)
    if (!headers) return
    for (var name in headers)
      if (headers.hasOwnProperty(name)) xhr.setRequestHeader(name, headers[name])
  }

  // Asynchronous request.
  //   method   "GET" | "POST" | "PATCH" | "DELETE" ...
  //   path     backend path starting with "/", query string included (callers encode their values)
  //   onDone   optional function(xhr), called once when the request has finished;
  //            xhr.status 0 means the backend did not answer
  //   body     optional string sent as is
  //   headers  optional { name: value }, e.g. { "Content-Type": "application/json" }
  // Returns the XMLHttpRequest (for getResponseHeader in onDone, or abort()).
  function request(method, path, onDone, body, headers) {
    var xhr = new XMLHttpRequest()
    xhr.onreadystatechange = function() {
      if (xhr.readyState !== XMLHttpRequest.DONE) return
      if (onDone) onDone(xhr)
    }
    xhr.open(method, root.baseUrl + path, true)
    _prepare(xhr, headers)
    if (body === undefined || body === null) xhr.send()
    else xhr.send(body)
    return xhr
  }

  // Synchronous request: blocks the UI thread. Only for the save on quit (the process ends right
  // after, so an async request would never be sent). Returns the finished XMLHttpRequest.
  function requestSync(method, path, body, headers) {
    var xhr = new XMLHttpRequest()
    xhr.open(method, root.baseUrl + path, false)
    _prepare(xhr, headers)
    if (body === undefined || body === null) xhr.send()
    else xhr.send(body)
    return xhr
  }
}
