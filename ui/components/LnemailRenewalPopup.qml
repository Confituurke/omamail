import QtQuick
import QtQuick.Controls as QQC
import qs.Commons
import qs.Ui

// Every connected LNemail mailbox's own expiry, and a way to pay for another
// year of it — LNemail's accounts run out, and nothing else in the header
// says so. A snapshot taken when the popup opens, not a live model: mailboxes
// are added and removed rarely enough that recomputing on open costs nothing
// and needs no watcher kept running while nobody is looking at this.
Item {
  id: root

  required property var service
  required property color textColor
  required property color dimColor
  required property color accentColor
  required property color dangerColor
  required property color popupBackgroundColor
  required property color popupBorderColor
  required property string panelFontFamily

  anchors.fill: parent
  z: 80

  readonly property bool opened: dialog.opened
  property var targets: []

  function lnemailHosts() {
    var accounts = service && service.accountList ? service.accountList.accounts : []
    var hosts = []
    for (var i = 0; i < accounts.length; i++) {
      if (accounts[i].provider !== "lnemail") continue
      var host = service.findAccount(accounts[i].id)
      if (host && host.auth) hosts.push(host)
    }
    return hosts
  }

  function openPopup() {
    var hosts = lnemailHosts()
    root.targets = hosts
    for (var i = 0; i < hosts.length; i++) hosts[i].auth.refreshAccountStatus()
    dialog.open()
  }

  function close() { dialog.close() }

  function expiryLabel(auth) {
    if (!auth || !auth.statusChecked) return "Checking expiry..."
    if (auth.statusError !== "") return auth.statusError
    if (auth.isExpired) return "Expired — renew to keep using this mailbox"
    if (auth.daysUntilExpiry < 0) return ""
    if (auth.daysUntilExpiry === 0) return "Renews today"
    if (auth.daysUntilExpiry === 1) return "Renews in 1 day"
    return "Renews in " + auth.daysUntilExpiry + " days"
  }

  function expiryUrgent(auth) {
    return !!auth && (auth.isExpired || (auth.daysUntilExpiry >= 0 && auth.daysUntilExpiry <= 30))
  }

  QQC.Popup {
    id: dialog
    anchors.centerIn: parent
    width: Math.min(Style.space(380), parent.width - Style.space(32))
    padding: Style.space(18)
    modal: true
    focus: true
    closePolicy: QQC.Popup.CloseOnEscape
    background: Rectangle {
      radius: Style.cornerRadius
      color: root.popupBackgroundColor
      border.width: 1
      border.color: root.popupBorderColor
    }
    contentItem: Column {
      width: dialog.width - dialog.leftPadding - dialog.rightPadding
      spacing: Style.space(16)

      Text {
        width: parent.width
        textFormat: Text.PlainText
        text: "LNemail"
        color: root.textColor
        font.family: root.panelFontFamily
        font.pixelSize: Style.font.heading
        font.bold: true
      }

      Text {
        width: parent.width
        visible: root.targets.length === 0
        textFormat: Text.PlainText
        text: "No LNemail mailbox is connected."
        color: root.dimColor
        font.family: root.panelFontFamily
        font.pixelSize: Style.font.bodySmall
        wrapMode: Text.WordWrap
      }

      Repeater {
        model: root.targets

        Column {
          id: target
          required property var modelData
          readonly property var auth: modelData ? modelData.auth : null

          width: parent.width
          spacing: Style.space(8)

          Text {
            width: parent.width
            textFormat: Text.PlainText
            text: target.auth ? target.auth.email : ""
            color: root.textColor
            font.family: root.panelFontFamily
            font.pixelSize: Style.font.bodySmall
            font.bold: true
          }

          Text {
            width: parent.width
            textFormat: Text.PlainText
            text: root.expiryLabel(target.auth)
            color: root.expiryUrgent(target.auth) ? root.dangerColor : root.dimColor
            font.family: root.panelFontFamily
            font.pixelSize: Style.font.caption
            wrapMode: Text.WordWrap
          }

          Column {
            width: parent.width
            visible: !!target.auth && target.auth.renewing
            spacing: Style.space(6)

            Text {
              width: parent.width
              textFormat: Text.PlainText
              text: target.auth && target.auth.renewalPriceSats > 0
                ? "Pay " + target.auth.renewalPriceSats + " sats to renew "
                  + target.auth.renewalYears + (target.auth.renewalYears === 1 ? " year:" : " years:")
                : "Pay this invoice to renew:"
              color: root.textColor
              font.family: root.panelFontFamily
              font.pixelSize: Style.font.caption
              wrapMode: Text.WordWrap
            }

            // Scanned with a phone's wallet rather than typed or copied by
            // hand. Rendered by the backend from the same invoice text the
            // field below carries, so the two can never disagree.
            Image {
              width: Math.min(parent.width, Style.space(200))
              height: width
              anchors.horizontalCenter: parent.horizontalCenter
              visible: !!target.auth && target.auth.renewalQrSvg !== ""
              fillMode: Image.PreserveAspectFit
              source: target.auth && target.auth.renewalQrSvg !== ""
                ? "data:image/svg+xml;utf8," + encodeURIComponent(target.auth.renewalQrSvg)
                : ""
            }

            TextField {
              width: parent.width
              readOnly: true
              text: target.auth ? target.auth.renewalPaymentRequest : ""
              foreground: root.textColor
              font.family: root.panelFontFamily
              font.pixelSize: Style.font.caption
            }

            Button {
              text: "Cancel"
              foreground: root.dimColor
              bordered: false
              fontSize: Style.font.caption
              onClicked: if (target.auth) target.auth.cancelRenewal()
            }
          }

          Text {
            width: parent.width
            visible: !!target.auth && !target.auth.renewing && target.auth.renewalError !== ""
            textFormat: Text.PlainText
            text: target.auth ? target.auth.renewalError : ""
            color: root.dangerColor
            font.family: root.panelFontFamily
            font.pixelSize: Style.font.caption
            wrapMode: Text.WordWrap
          }

          Button {
            visible: !!target.auth && !target.auth.renewing
            text: "Renew 1 year"
            foreground: root.textColor
            bordered: true
            fontSize: Style.font.caption
            enabled: !!target.auth && target.auth.statusChecked
            onClicked: if (target.auth) target.auth.renew(1)
          }
        }
      }

      Row {
        anchors.right: parent.right
        Button {
          text: "Close"
          foreground: root.textColor
          bordered: false
          fontSize: Style.font.bodySmall
          onClicked: dialog.close()
        }
      }
    }
  }
}
