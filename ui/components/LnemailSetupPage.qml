import QtQuick
import qs.Commons
import qs.Ui

// LNemail has two doors in, and neither is a password typed against a server
// this app already knows the address of: an existing account's access token,
// asked back from LNemail rather than trusted from whatever was pasted, or a
// brand new mailbox LNemail names itself once its signup invoice is paid.
Column {
  id: root

  required property var service
  required property color textColor
  required property color dimColor
  required property color dangerColor
  required property color accentColor
  required property string panelFontFamily
  property int accountCount: 1

  signal removeRequested()

  readonly property var auth: service ? service.auth : null
  readonly property bool signedIn: !!auth && auth.loggedIn
  readonly property bool busy: !!auth && auth.loginBusy
  readonly property bool creating: !!auth && auth.creatingAccount

  spacing: Style.space(16)

  function connect() {
    if (!service || !auth) return
    var token = tokenField.text.trim()
    if (token === "") {
      errorText.text = "Paste the access token from your LNemail account"
      return
    }
    errorText.text = ""
    service.configureCurrentAccountAndSignIn(
      { provider: "lnemail", label: nameField.value() }, token)
  }

  function createAccount() {
    if (!auth) return
    errorText.text = ""
    auth.createAccount()
  }

  Component.onCompleted: nameField.syncFromStore()

  Connections {
    target: root.auth
    ignoreUnknownSignals: true
    function onLastErrorChanged() {
      if (root.auth && root.auth.lastError !== "") errorText.text = root.auth.lastError
    }
  }

  ProviderHero {
    width: parent.width
    providerId: "lnemail"
    title: "Add an LNemail mailbox"
    detail: "A disposable email address paid for and read over the Lightning Network. No signup, no password — a token instead."
    textColor: root.textColor
    dimColor: root.dimColor
    panelFontFamily: root.panelFontFamily
    onWebsiteRequested: if (root.service) root.service.openProviderWebsite("lnemail")
  }

  Rectangle {
    width: parent.width
    visible: !!root.auth && root.auth.toolsChecked && root.auth.missingTools.length > 0
    implicitHeight: missingText.implicitHeight + Style.space(20)
    radius: Style.cornerRadius
    color: Style.normalFillFor(root.textColor, root.accentColor)
    border.width: 1
    border.color: Style.hoverBorderFor(root.textColor, root.accentColor)

    Text {
      id: missingText
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.margins: Style.space(12)
      anchors.verticalCenter: parent.verticalCenter
      text: root.auth ? "Install " + root.auth.missingTools.join(", ") + " first — it holds the token." : ""
      color: root.textColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.caption
      wrapMode: Text.WordWrap
    }
  }

  Text {
    id: errorText
    objectName: "lnemail-error"
    textFormat: Text.PlainText
    width: parent.width
    visible: text !== ""
    text: ""
    color: root.dangerColor
    font.family: root.panelFontFamily
    font.pixelSize: Style.font.caption
    wrapMode: Text.WordWrap
  }

  // -------------------------------------------------------- an existing mailbox

  Column {
    width: parent.width
    visible: !root.signedIn && !root.creating
    spacing: Style.space(10)

    AccountNameField {
      id: nameField
      service: root.service
      width: parent.width
      foreground: root.textColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.bodySmall
      onAccepted: tokenField.forceActiveFocus()
    }

    Text {
      width: parent.width
      text: "Already have an LNemail account?"
      color: root.textColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.bodySmall
      font.bold: true
    }

    TextField {
      id: tokenField
      objectName: "lnemail-token-field"
      width: parent.width
      password: true
      foreground: root.textColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.bodySmall
      placeholderText: "Access token"
      onAccepted: root.connect()
    }

    Button {
      objectName: "lnemail-connect"
      text: root.busy ? "Checking..." : "Connect this mailbox"
      enabled: !root.busy && tokenField.text.trim() !== ""
      foreground: root.textColor
      bordered: true
      fontSize: Style.font.bodySmall
      onClicked: root.connect()
    }
  }

  // ------------------------------------------------------------ a new mailbox

  Column {
    width: parent.width
    visible: !root.signedIn && !root.creating
    spacing: Style.space(6)

    Text {
      width: parent.width
      text: "Or create a new one"
      color: root.textColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.bodySmall
      font.bold: true
    }

    Text {
      width: parent.width
      text: "LNemail creates a random address for 1000 sats a year, paid over the Lightning Network from any wallet. Sending costs about 100 sats per email; reading is free."
      color: root.dimColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.caption
      wrapMode: Text.WordWrap
    }

    Button {
      objectName: "lnemail-create-account"
      text: "Create a new mailbox..."
      enabled: !root.busy
      foreground: root.textColor
      bordered: true
      fontSize: Style.font.bodySmall
      onClicked: root.createAccount()
    }
  }

  // ------------------------------------------------------ awaiting payment

  Column {
    width: parent.width
    visible: root.creating
    spacing: Style.space(10)

    Text {
      width: parent.width
      text: root.auth && root.auth.priceSats > 0
        ? "Pay " + root.auth.priceSats + " sats to activate this mailbox:"
        : "Pay this invoice to activate the mailbox:"
      color: root.textColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.bodySmall
      wrapMode: Text.WordWrap
    }

    TextField {
      objectName: "lnemail-invoice"
      width: parent.width
      readOnly: true
      text: root.auth ? root.auth.paymentRequest : ""
      foreground: root.textColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.caption
    }

    Text {
      width: parent.width
      text: "Pay it from any Lightning wallet. This page updates on its own once the payment is seen."
      color: root.dimColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.caption
      wrapMode: Text.WordWrap
    }

    Row {
      spacing: Style.space(8)

      Button {
        objectName: "lnemail-cancel-create"
        text: "Cancel"
        foreground: root.dimColor
        bordered: false
        fontSize: Style.font.bodySmall
        onClicked: if (root.auth) root.auth.cancelAccountCreation()
      }
    }
  }

  // --------------------------------------------------------------- signed in

  Column {
    width: parent.width
    visible: root.signedIn
    spacing: Style.space(6)

    Text {
      width: parent.width
      text: "Signed in as"
      color: root.dimColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.caption
    }

    Text {
      width: parent.width
      textFormat: Text.PlainText
      text: root.auth ? root.auth.email : ""
      color: root.textColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.bodySmall
      font.bold: true
    }
  }

  Row {
    spacing: Style.space(8)

    Button {
      visible: root.signedIn
      text: "Sign out"
      foreground: root.textColor
      bordered: true
      fontSize: Style.font.bodySmall
      onClicked: if (root.service) root.service.signOut()
    }

    Button {
      visible: root.accountCount > 1
      text: "Remove account"
      foreground: root.dangerColor
      bordered: false
      fontSize: Style.font.bodySmall
      onClicked: root.removeRequested()
    }
  }
}
