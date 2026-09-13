import QtQuick
import Quickshell
import Quickshell.Io

import "Credentials.js" as Credentials
import "Secrets.js" as Secrets

// LNemail's own sign-in: either an existing account's access token pasted in,
// or a brand new mailbox created here and paid for out of band. The token is
// LNemail's only credential — it is never refreshed or rotated — so unlike an
// OAuth manager this holds no session to renew, only the one secret
// secret-tool keeps, exactly as ImapAuth and JmapAuth do for theirs.
Item {
  id: root

  visible: false
  width: 0
  height: 0

  required property string pluginDir
  property var backend: null

  property string accountId: ""

  // Learned from the server, never typed: a new mailbox's address is
  // LNemail's own choice, and a pasted token's address is asked for — `GET
  // /account` — rather than trusted from whatever the user typed elsewhere.
  property string email: ""

  readonly property string authMode: "password"
  readonly property bool configured: email !== ""

  property string token: ""
  property bool tokenChecked: false
  readonly property bool loggedIn: configured && token !== ""

  // The same names `ImapAuth` and `JmapAuth` expose, because `MailAccount`
  // reads them without knowing which provider it has.
  readonly property bool credentialsPresent: configured
  property bool loginBusy: false
  readonly property bool sessionBusy: secretLookup.running || keyringStore.running
  property string lastError: ""

  readonly property var requiredTools: ["secret-tool"]
  property var missingTools: []
  property bool toolsChecked: false
  readonly property bool toolsPresent: toolsChecked && missingTools.length === 0

  property var credentialWaiters: []
  property bool lookupHandled: false
  property string pendingToken: ""

  // The pending signup invoice, while a new mailbox waits to be paid for out
  // of band. Nothing here ever touches a wallet: this only displays it and
  // polls whether someone else has paid it.
  property string paymentRequest: ""
  property string paymentHash: ""
  property int priceSats: 0
  property bool creatingAccount: false

  signal loginSucceeded()
  signal loggedOut()
  signal sessionUnavailable(string reason)
  signal credentialsSaved()

  function friendlyError(message, fallback) {
    var code = String(message || "")
    if (code === "lnemail_unauthorized") return "LNemail rejected that token"
    if (code === "lnemail_not_found") return "LNemail found no such mailbox"
    if (code === "lnemail_rate_limited") return "LNemail asked to slow down. Try again shortly"
    if (code === "" || code.indexOf("lnemail_") === 0) return fallback
    return code
  }

  function finishWaiters(value, error) {
    var pending = credentialWaiters.slice()
    credentialWaiters = []
    for (var i = 0; i < pending.length; i++) {
      try { pending[i](value || "", error || "") }
      catch (e) { /* consumers own their callback errors */ }
    }
  }

  // The one entry point the client uses to get the bearer token.
  function withCredentials(callback) {
    if (typeof callback !== "function") return
    if (!configured) {
      callback("", "Add or create this mailbox first")
      return
    }
    if (token !== "") {
      callback(token, "")
      return
    }
    if (tokenChecked) {
      callback("", "No token saved for this mailbox. Sign in again")
      return
    }

    var next = credentialWaiters.slice()
    next.push(callback)
    credentialWaiters = next
    if (secretLookup.running) return
    startSecretLookup()
  }

  function restoreSession() {
    if (!configured) {
      tokenChecked = true
      return
    }
    if (secretLookup.running) return
    startSecretLookup()
  }

  function startSecretLookup() {
    var attributes = Credentials.lnemailKeyringAttributes(accountId)
    if (attributes.length === 0) {
      handleSecretLookup("")
      return
    }
    lookupHandled = false
    secretLookup.command = ["secret-tool", "lookup"].concat(attributes)
    secretLookup.running = true
  }

  function handleSecretLookup(line) {
    if (lookupHandled) return
    lookupHandled = true
    tokenChecked = true
    var value = String(line || "")
    if (value === "") {
      finishWaiters("", "No token saved for this mailbox. Sign in again")
      // Only a mailbox that is otherwise ready to go is worth complaining
      // about: an account still being created has no token by design.
      if (configured) sessionUnavailable("Sign in to this mailbox")
      return
    }
    token = value
    finishWaiters(token, "")
    loginSucceeded()
  }

  // Path one: an existing LNemail account's access token, pasted in. Verified
  // by asking who it belongs to, rather than saved first and failing silently
  // on the next request.
  function signIn(pastedToken) {
    if (root.creatingAccount) return false
    var value = String(pastedToken || "").trim()
    if (value === "") {
      lastError = "Paste the access token from your LNemail account"
      return false
    }
    lastError = ""
    loginBusy = true
    pendingToken = value
    verifyToken(value)
    return true
  }

  function verifyToken(value) {
    if (!backend) {
      completeSignIn(false, "", "Mail backend unavailable")
      return
    }
    backend.call("lnemail.account", { token: value }, function(result, error) {
      if (error) {
        completeSignIn(false, "", friendlyError(error.message, "That token was refused"))
        return
      }
      var learned = String((result || {}).email_address || "")
      if (learned === "") {
        completeSignIn(false, "", "LNemail did not name an address for that token")
        return
      }
      completeSignIn(true, learned, "")
    })
  }

  function completeSignIn(ok, learnedEmail, error) {
    loginBusy = false
    if (!ok) {
      pendingToken = ""
      lastError = error || "That token was refused"
      return
    }
    email = learnedEmail
    token = pendingToken
    pendingToken = ""
    tokenChecked = true
    lastError = ""
    storeToken()
    credentialsSaved()
    loginSucceeded()
  }

  // Path two: a brand new mailbox. `POST /email` needs no account and no
  // token — there is neither yet — and hands back a Lightning invoice this
  // only displays and polls the status of.
  function createAccount() {
    if (creatingAccount || loginBusy) return
    lastError = ""
    creatingAccount = true
    paymentRequest = ""
    paymentHash = ""
    priceSats = 0
    if (!backend) {
      creatingAccount = false
      lastError = "Mail backend unavailable"
      return
    }
    backend.call("lnemail.createAccount", {}, function(result, error) {
      if (!root.creatingAccount) return // cancelled meanwhile
      if (error) {
        creatingAccount = false
        lastError = friendlyError(error.message, "Could not reach LNemail")
        return
      }
      var value = result || {}
      var invoice = String(value.payment_request || "")
      var hash = String(value.payment_hash || "")
      if (invoice === "" || hash === "") {
        creatingAccount = false
        lastError = "LNemail did not return an invoice"
        return
      }
      paymentRequest = invoice
      paymentHash = hash
      priceSats = Number(value.price_sats) || 0
      paymentPoll.running = true
    })
  }

  function cancelAccountCreation() {
    paymentPoll.running = false
    creatingAccount = false
    paymentRequest = ""
    paymentHash = ""
    priceSats = 0
  }

  function pollPayment() {
    if (!backend || paymentHash === "" || !creatingAccount) return
    var askedHash = paymentHash
    backend.call("lnemail.paymentStatus", { paymentHash: askedHash }, function(result, error) {
      // The invoice this answers for may have been cancelled or replaced
      // while the request was in flight.
      if (!root.creatingAccount || root.paymentHash !== askedHash) return
      if (error) return // a transient failure is retried on the next tick
      var value = result || {}
      var status = String(value.payment_status || "")
      if (status === "paid") {
        paymentPoll.running = false
        creatingAccount = false
        var learned = String(value.email_address || "")
        var granted = String(value.access_token || "")
        if (learned === "" || granted === "") {
          lastError = "LNemail confirmed payment but did not return a mailbox"
          return
        }
        pendingToken = granted
        completeSignIn(true, learned, "")
      } else if (status === "expired" || status === "failed") {
        paymentPoll.running = false
        creatingAccount = false
        paymentRequest = ""
        paymentHash = ""
        lastError = status === "expired"
          ? "That invoice expired before it was paid. Create a new one"
          : "The payment failed. Create a new invoice"
      }
      // "pending" says nothing and is silent: the poll simply continues.
    })
  }

  function storeToken() {
    var attributes = Credentials.lnemailKeyringAttributes(accountId)
    if (attributes.length === 0 || token === "") return
    keyringWriteSecret = token
    keyringStore.command = [pluginDir + "/scripts/keyring-store.sh"].concat(attributes)
    keyringStore.running = true
  }

  property string keyringWriteSecret: ""

  function logout() {
    token = ""
    pendingToken = ""
    tokenChecked = true
    var attributes = Credentials.lnemailKeyringAttributes(accountId)
    if (attributes.length > 0) {
      keyringClear.command = ["secret-tool", "clear"].concat(attributes)
      keyringClear.running = true
    }
    loggedOut()
  }

  // A LNemail token does not expire — but a server that has started rejecting
  // it should not be asked a hundred more times with the same value.
  function invalidateAccessToken() {
    token = ""
    tokenChecked = false
  }

  // Neither Gmail's manager nor `MailAccount` should have to ask which
  // provider it holds before calling one.
  function beginLogin() { /* the setup form drives sign-in, not a browser */ }
  function cancelLogin() {
    loginBusy = false
    cancelAccountCreation()
  }

  onAccountIdChanged: {
    // A different mailbox has a different token. Dropping the one in memory
    // is what stops an account rename from leaving the previous account's
    // credential in front of the new one's server.
    token = ""
    tokenChecked = false
    lookupHandled = false
  }

  Component.onCompleted: {
    toolProbe.command = ["sh", "-c",
      "for tool in secret-tool; do command -v \"$tool\" >/dev/null 2>&1 || echo \"$tool\"; done"]
    toolProbe.running = true
  }

  Timer {
    id: paymentPoll
    interval: 3000
    repeat: true
    onTriggered: root.pollPayment()
  }

  Process {
    id: toolProbe
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var missing = String(text || "").split("\n")
        var found = []
        for (var i = 0; i < missing.length; i++) {
          var name = missing[i].trim()
          if (name) found.push(name)
        }
        root.missingTools = found
        root.toolsChecked = true
      }
    }
  }

  Process {
    id: secretLookup
    stdout: StdioCollector { id: secretOutput; waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onExited: function(exitCode) {
      // One trailing newline is the pipe's; everything else is the secret.
      var value = exitCode === 0 ? Secrets.fromKeyring(secretOutput.text) : ""
      root.handleSecretLookup(value)
    }
  }

  Process {
    id: keyringStore
    stdinEnabled: true
    stdout: StdioCollector { waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
    onStarted: {
      write(root.keyringWriteSecret + "\n")
      root.keyringWriteSecret = ""
    }
    onExited: function(exitCode) {
      root.keyringWriteSecret = ""
      if (exitCode !== 0)
        root.lastError = "Signed in, but the token could not be saved. "
          + "You may need to enter it again after a restart"
    }
  }

  Process {
    id: keyringClear
    stdout: StdioCollector { waitForEnd: true }
    stderr: StdioCollector { waitForEnd: true }
  }
}
