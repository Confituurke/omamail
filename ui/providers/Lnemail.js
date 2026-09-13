.pragma library

// Provider presentation only. NativeDomain.js supplies all mail capabilities
// and mailbox queries; providers.resolve builds dynamic queries in Rust.
//
// A flat inbox with no folders: LNemail's API lists one box and nothing else,
// so there is no Drafts, Archive, Junk or Trash to offer — offering one that
// fails is worse than not drawing it at all. Sent is the one exception, and
// an honest one: it opens LNemail's own status log of recent outgoing
// payments, a destination and a subject with no body behind them, never a
// copy of what was actually sent.

var ID = "lnemail"

var NAME = "LNemail"

var SUMMARY = "A disposable mailbox paid for and read over the Lightning Network."

var AUTH = "password"

var MAILBOXES = [
  {
    "key": "inbox",
    "label": "Inbox",
    "icon": "inbox"
  },
  {
    "key": "unread",
    "label": "Unread",
    "icon": "unread"
  },
  {
    "key": "sent",
    "label": "Sent",
    "icon": "sent"
  }
]
