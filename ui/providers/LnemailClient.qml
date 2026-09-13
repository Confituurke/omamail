import QtQuick

// LNemail's own reader. Rust already hands back the shared Gmail message
// resource for every list and read — see providers::lnemail_resource — so
// this is presentation and dispatch only, the way HeyClient is: no protocol
// of its own, no parsing, just the backend calls a flat, threadless, label-
// less mailbox actually needs.
Item {
  id: root

  visible: false
  width: 0
  height: 0

  required property var auth
  property var backend: null

  property int inFlight: 0
  readonly property bool busy: inFlight > 0

  // What a listing has already told this client about a message: enough for
  // a row, and enough that a non-full read need not ask the server again.
  property var messageResources: ({})

  function newHandle() { return { aborted: false, children: [] } }
  function abortRequest(handle) {
    if (!handle) return
    handle.aborted = true
    var children = handle.children || []
    for (var i = 0; i < children.length; i++) abortRequest(children[i])
  }

  function backendError(error) {
    var code = String((error && error.message) || "")
    if (code === "lnemail_unauthorized") return "LNemail rejected the saved token. Sign in again"
    if (code === "lnemail_not_found") return "That message is no longer in the mailbox"
    if (code === "lnemail_rate_limited") return "LNemail asked to slow down. Try again shortly"
    if (code === "lnemail_recipient_required") return "Add a recipient before sending"
    if (code === "lnemail_single_recipient_only") return "LNemail can send to one recipient at a time"
    if (code === "lnemail_sent_log_readonly") return "LNemail's own send log can't be edited or deleted here"
    return "Mail request failed"
  }

  function call(method, params, callback, handle) {
    var owner = auth
    var account = owner ? String(owner.accountId || "") : ""
    if (!backend || !owner || !owner.loggedIn || account === "") {
      Qt.callLater(function() {
        if (!handle.aborted && typeof callback === "function") callback(null, "Sign in to this mailbox first")
      })
      return handle
    }
    var request = Object.assign({}, params, { accountId: account })
    root.inFlight++
    backend.call(method, request, function(result, error) {
      root.inFlight = Math.max(0, root.inFlight - 1)
      if (handle.aborted) return
      var current = owner === auth && String(owner.accountId || "") === account
      if (!current) {
        if (typeof callback === "function") callback(null, "The account changed during this request")
        return
      }
      if (typeof callback === "function") callback(result, error ? root.backendError(error) : "")
    })
    return handle
  }

  function rememberResources(list) {
    var next = {}
    for (var key in messageResources) next[key] = messageResources[key]
    for (var i = 0; i < list.length; i++) next[String(list[i].id)] = list[i]
    messageResources = next
  }

  // A flat inbox has one listing and nothing to page through: LNemail's own
  // `GET /emails` answers with everything in one call. "sent" is the one
  // query this reads: LNemail's status log for recent outgoing payments
  // instead of the inbox, and Rust is what tells the two apart.
  function listMessages(query, maxResults, pageToken, callback, progress) {
    var handle = newHandle()
    call("lnemail.list", { query: String(query || "").trim() }, function(result, error) {
      if (error || !result) {
        if (typeof callback === "function") callback(null, error)
        return
      }
      root.rememberResources(result.messages || [])
      var page = { ids: result.ids, threadIds: result.threadIds,
        nextPageToken: "", estimate: result.estimate }
      if (typeof progress === "function" && page.ids.length > 0) progress(page)
      if (typeof callback === "function") callback(page, "")
    }, handle)
    return handle
  }

  // Nothing asks the server again for what the listing already gave: a
  // summary is what `rememberResources` kept.
  function getSummaries(ids, callback) {
    var handle = newHandle()
    Qt.callLater(function() {
      if (!handle.aborted && typeof callback === "function") callback([], "")
    })
    return handle
  }

  function getMessages(ids, full, callback, existingHandle, progress) {
    var handle = existingHandle || newHandle()
    var list = Array.isArray(ids) ? ids : []
    if (list.length === 0) {
      Qt.callLater(function() {
        if (!handle.aborted && typeof callback === "function") callback([], "")
      })
      return handle
    }
    var results = []
    var pending = list.length
    var failure = ""
    function settle() {
      if (pending > 0 || handle.aborted) return
      if (typeof callback === "function") callback(results, failure)
    }
    for (var i = 0; i < list.length; i++) {
      (function(id) {
        if (full === true) {
          call("lnemail.read", { id: id }, function(result, error) {
            pending--
            if (!handle.aborted) {
              if (!error && result) {
                results.push(result)
                root.rememberResources([result])
                if (typeof progress === "function") progress([result])
              } else if (error) failure = error
            }
            settle()
          }, handle)
        } else {
          var known = root.messageResources[String(id)]
          if (known) results.push(known)
          pending--
          Qt.callLater(settle)
        }
      })(list[i])
    }
    return handle
  }

  function getMessage(id, full, callback) {
    return getMessages([id], full, function(messages, error) {
      if (typeof callback !== "function") return
      if (error || messages.length === 0) callback(null, error || "That message is no longer in the mailbox")
      else callback(messages[0], "")
    })
  }

  function getAttachment(messageId, attachmentId, callback) {
    return call("lnemail.attachment", { id: messageId, attachmentId: attachmentId },
      function(result, error) {
        if (typeof callback === "function") callback(error ? "" : (result ? result.data : ""), error)
      }, newHandle())
  }

  // LNemail has no folders of its own, so nothing here has a name to give.
  function getLabels(callback) {
    var handle = newHandle()
    Qt.callLater(function() { if (!handle.aborted && typeof callback === "function") callback([], "") })
    return handle
  }

  function getProfile(callback) {
    var handle = newHandle()
    var address = auth ? String(auth.email || "") : ""
    Qt.callLater(function() {
      if (handle.aborted || typeof callback !== "function") return
      callback({ email: address, messagesTotal: 0, threadsTotal: 0, historyId: "" }, "")
    })
    return handle
  }

  function getSendAs(callback) {
    var handle = newHandle()
    var address = auth ? String(auth.email || "") : ""
    Qt.callLater(function() {
      if (handle.aborted || typeof callback !== "function") return
      if (address === "") { callback([], ""); return }
      callback([{ email: address, displayName: "", isPrimary: true, isDefault: true }], "")
    })
    return handle
  }

  function refuse(callback, message) {
    var handle = newHandle()
    if (typeof callback === "function")
      Qt.callLater(function() { if (!handle.aborted) callback(null, message) })
    return handle
  }

  // The capability is off, so no button reaches these; they exist so every
  // client answers the same calls.
  function modifyMessage(id, addLabelIds, removeLabelIds, callback) {
    return refuse(callback, "LNemail has no labels or flags to change")
  }
  function batchModify(ids, addLabelIds, removeLabelIds, callback) {
    return refuse(callback, "LNemail has no labels or flags to change")
  }
  function createLabel(name, callback) { return refuse(callback, "LNemail has no folders of its own") }
  function renameLabel(id, name, callback) { return refuse(callback, "LNemail has no folders of its own") }
  function deleteLabel(id, callback) { return refuse(callback, "LNemail has no folders of its own") }
  function saveDraft(payload, callback) { return refuse(callback, "LNemail keeps no drafts") }
  function deleteDraft(messageId, callback) { return refuse(callback, "LNemail keeps no drafts") }

  // There is no Trash mailbox to move a message into: deleting is the only
  // verb LNemail has, and it is final.
  function trashMessage(id, callback) {
    var ids = Array.isArray(id) ? id : [id]
    if (ids.length === 1) return call("lnemail.delete", { id: ids[0] }, callback, newHandle())
    return call("lnemail.deleteMany", { ids: ids }, callback, newHandle())
  }

  function untrashMessage(id, callback) {
    return refuse(callback, "LNemail deletes for good — there is no Trash to restore a message from")
  }

  function sendMessage(payload, callback) {
    return call("lnemail.send", { raw: String((payload && payload.raw) || "") }, callback, newHandle())
  }
}
