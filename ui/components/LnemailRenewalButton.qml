import QtQuick

import "../account/LnemailRenewal.js" as LnemailRenewal

// LNemail is the one mailbox that runs out: 1000 sats buys it a year. Shown
// whenever one is in view — alone or among "All mail" — because its expiry
// is worth a standing glance, not something to notice only once it has
// already lapsed. What it says and how urgent it looks both follow from
// LnemailRenewal.js reading whichever single LNemail mailbox is current.
IconButton {
  id: root

  property var service: null
  property color dimColor
  property color dangerColor
  readonly property var auth: LnemailRenewal.currentAuth(service)

  signal openRequested()

  objectName: "lnemail-renewal-button"
  iconName: "renew"
  tooltipText: LnemailRenewal.tooltip(auth)
  foreground: LnemailRenewal.urgent(auth) ? dangerColor : dimColor
  onClicked: root.openRequested()
}
