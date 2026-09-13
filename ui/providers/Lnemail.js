.pragma library

// Provider presentation only. NativeDomain.js supplies all mail capabilities
// and mailbox queries; providers.resolve builds dynamic queries in Rust.
//
// A flat inbox with no folders: LNemail's API lists one box and nothing else,
// so there is no Sent, Drafts, Archive, Junk or Trash to offer — offering one
// that fails is worse than not drawing it at all.

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
  }
]
