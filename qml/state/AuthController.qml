import QtQuick
import Quickshell
import Quickshell.Io

// Login state: Microsoft device-code flow and Gmail browser flow. The Rust backend writes auth_state.json
// and device_code.json into the config dir; we watch those files (no polling)
// and POST login/logout intents to the backend.
Item {
  id: root

  property string configDir: ""

  property bool isAuthenticated: false
  property bool showModal: false
  property string userCode: ""
  property string verificationUri: ""
  property int secondsRemaining: 0
  property string errorMessage: ""

  // Gmail sign-in: the user types an address (needsEmail), the backend opens the browser
  // and publishes the Google URL (googleUrl, shown as a fallback) until sign-in completes.
  property string loginProvider: ""
  property bool needsEmail: false
  property string googleUrl: ""

  // Bumped by the backend after each completed login/sign-out (also while another
  // account is already signed in, when isAuthenticated doesn't change).
  property double loginSerial: 0

  // Set when a sign-in was for a mailbox that is already signed in:
  // { pending_id, account_id, email } — the UI must ask before anything is replaced.
  property var reauthPrompt: null

  function answerReauth(decision) {
    var prompt = root.reauthPrompt
    if (!prompt) return
    root.reauthPrompt = null
    Backend.request("POST", "/auth/confirm?pending=" + encodeURIComponent(prompt.pending_id)
                            + "&decision=" + decision,
                    function(xhr) { root.accountsChanged() })
  }
  signal accountsChanged()

  function startLogin(provider) {
    userCode = ""
    verificationUri = ""
    googleUrl = ""
    secondsRemaining = 0
    errorMessage = ""
    loginProvider = provider || "exchange"
    showModal = true

    // Gmail: ask for the address first; submitEmail() starts the browser sign-in.
    if (loginProvider === "gmail") {
      needsEmail = true
      return
    }
    needsEmail = false

    Backend.request("POST", "/auth/login?provider=" + encodeURIComponent(provider || "exchange"), function(xhr) {
      if (xhr.status === 200) codeFile.reload()
      else root.errorMessage = "Failed to start authentication (backend not ready?)"
    })
  }

  // Gmail step 2: the address was entered. The backend opens Google's sign-in page in the
  // browser (address pre-filled) and waits for it; auth_state.json reports the outcome.
  function submitEmail(email) {
    var addr = (email || "").trim()
    if (addr.indexOf("@") < 1) { errorMessage = "Enter your Gmail address"; return }
    errorMessage = ""
    needsEmail = false
    googleUrl = ""
    secondsRemaining = 300
    countdown.restart()
    Backend.request("POST", "/auth/login?provider=gmail&email=" + encodeURIComponent(addr), function(xhr) {
      if (xhr.status === 200) googleFile.reload()
      else root.errorMessage = "Failed to start authentication (backend not ready?)"
    })
  }

  // Log in an existing account: the backend reuses its kept token, or says a device
  // flow is needed (then we run the normal login for that provider).
  function loginAccount(accountId, provider) {
    Backend.request("POST", "/accounts/login?account=" + encodeURIComponent(accountId), function(xhr) {
      var res = {}
      try { res = JSON.parse(xhr.responseText) } catch (e) {}
      if (res.result === "login_required") root.startLogin(res.provider || provider)
      else root.accountsChanged()
    })
  }

  // Log one account out (sync stops; its token and cached data are kept).
  function logout(accountId) {
    postAccountAction("/auth/logout", accountId)
  }

  // Delete an account: its keyring token and everything cached for it.
  function removeAccount(accountId) {
    postAccountAction("/accounts/remove", accountId)
  }

  function postAccountAction(path, accountId) {
    Backend.request("POST", path + "?account=" + encodeURIComponent(accountId),
                    function(xhr) { root.accountsChanged() })
    // authFile watcher picks up the resulting state change.
  }

  function cancel() {
    if (loginProvider === "gmail" && !needsEmail) {
      // Frees the backend's one-shot loopback listener.
      Backend.request("POST", "/auth/cancel")
    }
    showModal = false
    needsEmail = false
    googleUrl = ""
    countdown.stop()
  }

  function applyAuthState(raw) {
    try {
      var data = JSON.parse(raw)
      var was = root.isAuthenticated
      root.isAuthenticated = data.is_authenticated === true
      if (data.error) { root.errorMessage = data.error; countdown.stop() }
      if (data.confirm_reauth) {
        root.reauthPrompt = data.confirm_reauth
        root.showModal = false
        countdown.stop()
        root.userCode = ""
      } else {
        root.reauthPrompt = null
      }
      var serial = data.login_serial || 0
      if (serial !== root.loginSerial) {
        root.loginSerial = serial
        if (serial > 0) {
          if (root.isAuthenticated && !data.error) { root.showModal = false; countdown.stop(); root.userCode = ""; root.googleUrl = "" }
          root.accountsChanged()
        }
      }
      if (!was && root.isAuthenticated) {
        root.showModal = false
        countdown.stop()
        root.userCode = ""
        root.googleUrl = ""
        root.errorMessage = ""
      }
    } catch (e) { /* partial write; the next change event re-reads */ }
  }

  function applyDeviceCode(raw) {
    if (!root.showModal) return
    try {
      var data = JSON.parse(raw)
      if (data.user_code && data.user_code !== root.userCode) {
        root.userCode = data.user_code
        root.verificationUri = data.verification_uri || "https://microsoft.com/devicelogin"
        root.secondsRemaining = data.expires_in || 900
        countdown.restart()
      }
    } catch (e) { /* not ready yet */ }
  }

  FileView {
    id: authFile
    path: root.configDir + "/auth_state.json"
    watchChanges: true
    printErrors: false
    onLoaded: root.applyAuthState(text())
    onFileChanged: reload()
    onLoadFailed: root.isAuthenticated = false
  }

  function applyGoogleLogin(raw) {
    if (!root.showModal || root.loginProvider !== "gmail" || root.needsEmail) return
    try {
      var data = JSON.parse(raw)
      // Ignore a leftover file from an earlier attempt.
      if (!data.auth_url || (data.started || 0) < Date.now() / 1000 - 30) return
      root.googleUrl = data.auth_url
      root.secondsRemaining = data.expires_in || 300
      countdown.restart()
    } catch (e) { /* not ready yet */ }
  }

  FileView {
    id: googleFile
    path: root.configDir + "/google_login.json"
    watchChanges: true
    printErrors: false
    onLoaded: root.applyGoogleLogin(text())
    onFileChanged: reload()
  }

  FileView {
    id: codeFile
    path: root.configDir + "/device_code.json"
    watchChanges: true
    printErrors: false
    onLoaded: root.applyDeviceCode(text())
    onFileChanged: reload()
  }

  Timer {
    id: countdown
    interval: 1000
    repeat: true
    onTriggered: {
      if (root.secondsRemaining > 0) {
        root.secondsRemaining -= 1
      } else {
        root.errorMessage = root.loginProvider === "gmail" ? "Sign-in timed out. Please try again." : "Device code expired. Please try again."
        stop()
      }
    }
  }
}
