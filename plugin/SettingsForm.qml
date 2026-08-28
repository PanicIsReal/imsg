import QtQuick
import qs.Commons
import qs.Ui

Column {
  id: root
  spacing: Style.space(12)

  property string serverUrl: ""
  property bool passwordSet: false
  property string session: "unconfigured"
  property bool saving: false
  property color foreground: Color.foreground
  property string fontFamily: Style.font.family
  property string lastError: ""
  property bool webhookEnabled: false
  property int webhookPort: 18792
  property string webhookServeUrl: ""
  property bool webhookListening: false
  property bool webhookRegistered: false
  property string webhookCopyUrl: ""
  property bool helpOpen: false

  readonly property bool editing: urlField.activeFocus || passwordField.activeFocus
  readonly property color dim: Qt.darker(foreground, 1.4)
  readonly property color actionFill: Qt.rgba(0, 0, 0, 0.35)
  readonly property string sessionCaption: {
    if (root.session === "live") return "Connected"
    if (root.session === "connecting") return "Connecting"
    if (root.session === "down") return "Link is down"
    return "Not configured"
  }

  signal saveRequested(string url, string password)
  signal reconnectRequested()
  signal webhookSaveRequested(bool enabled, int port, string serveUrl)
  signal webhookRegisterRequested()
  signal webhookRotateRequested()
  signal webhookCopyRequested()
  signal webhookServeRequested(int port)

  onServerUrlChanged: {
    if (!urlField.activeFocus) urlField.text = root.serverUrl
  }

  Text {
    width: parent.width
    text: "BlueBubbles URL and password. Saved in the system keyring."
    color: root.dim
    font.family: root.fontFamily
    font.pixelSize: Style.font.body
    wrapMode: Text.WordWrap
  }

  TextField {
    id: urlField
    width: parent.width
    foreground: root.foreground
    text: root.serverUrl
    placeholderText: "http://100.x.x.x:1234"
    enabled: !root.saving
  }

  TextField {
    id: passwordField
    width: parent.width
    foreground: root.foreground
    password: true
    placeholderText: root.passwordSet ? "unchanged" : "BlueBubbles password"
    enabled: !root.saving
  }

  Text {
    width: parent.width
    visible: root.session.length > 0
    text: root.sessionCaption
    color: root.dim
    font.family: root.fontFamily
    font.pixelSize: Style.font.caption
    wrapMode: Text.WordWrap
  }

  Text {
    width: parent.width
    visible: root.lastError && root.lastError.length > 0 && root.session === "down"
    text: root.lastError
    color: root.dim
    font.family: root.fontFamily
    font.pixelSize: Style.font.caption
    wrapMode: Text.WordWrap
  }

  Row {
    width: parent.width
    spacing: Style.space(8)

    Button {
      text: root.saving ? "Saving…" : "Save"
      foreground: root.foreground
      fontFamily: root.fontFamily
      enabled: !root.saving && urlField.text.trim().length > 0
      onClicked: {
        var url = urlField.text
        var password = passwordField.text
        passwordField.text = ""
        root.saveRequested(url, password)
      }
    }

    Button {
      text: "Reconnect"
      foreground: root.foreground
      fontFamily: root.fontFamily
      bordered: true
      enabled: !root.saving
      onClicked: root.reconnectRequested()
    }
  }

  Text {
    width: parent.width
    topPadding: Style.space(8)
    text: "Webhook"
    color: root.foreground
    font.family: root.fontFamily
    font.pixelSize: Style.font.body
    font.bold: true
  }

  Text {
    width: parent.width
    text: root.webhookEnabled
      ? (root.webhookListening
        ? (root.webhookRegistered ? "Listening. Registered with BlueBubbles. Poll is off." : "Listening. Register with BlueBubbles to receive events. Poll is off.")
        : "Enabled. Waiting for the listener.")
      : "Off. New iMessages use a 2s poll."
    color: root.dim
    font.family: root.fontFamily
    font.pixelSize: Style.font.caption
    wrapMode: Text.WordWrap
  }

  Button {
    width: parent.width
    text: root.helpOpen ? "Hide webhook help" : "Why a webhook?"
    foreground: root.foreground
    fontFamily: root.fontFamily
    bordered: true
    onClicked: root.helpOpen = !root.helpOpen
  }

  Text {
    width: parent.width
    visible: root.helpOpen
    text: "BlueBubbles pokes this machine. We then pull the real message with your password. Listen on localhost. Publish with tailscale serve, not Funnel. Restrict the Serve ACL to your Mac."
    color: root.dim
    font.family: root.fontFamily
    font.pixelSize: Style.font.caption
    wrapMode: Text.WordWrap
  }

  TextField {
    id: portField
    width: parent.width
    foreground: root.foreground
    text: String(root.webhookPort)
    placeholderText: "Port (default 18792)"
    enabled: !root.saving
  }

  TextField {
    id: serveField
    width: parent.width
    foreground: root.foreground
    text: root.webhookServeUrl
    placeholderText: "https://<linux>.<tailnet>.ts.net"
    enabled: !root.saving
  }

  onWebhookPortChanged: {
    if (!portField.activeFocus) portField.text = String(root.webhookPort)
  }
  onWebhookServeUrlChanged: {
    if (!serveField.activeFocus) serveField.text = root.webhookServeUrl
  }

  function webhookPortValue() {
    var port = parseInt(portField.text, 10)
    if (!port || port < 1 || port > 65535) port = 18792
    return port
  }

  Button {
    width: parent.width
    text: "Publish with Tailscale"
    foreground: root.foreground
    fontFamily: root.fontFamily
    bordered: true
    background: root.actionFill
    enabled: !root.saving
    opacity: enabled ? 1 : 0.4
    onClicked: {
      var port = root.webhookPortValue()
      root.webhookSaveRequested(root.webhookEnabled, port, serveField.text.trim())
      root.webhookServeRequested(port)
    }
  }

  Toggle {
    width: parent.width
    label: "Toggle"
    description: "Enable webhook"
    checked: root.webhookEnabled
    foreground: root.foreground
    fontFamily: root.fontFamily
    opacity: root.saving ? 0.55 : 1
    onClicked: {
      if (root.saving) return
      root.webhookSaveRequested(!root.webhookEnabled, root.webhookPortValue(), serveField.text.trim())
    }
  }

  Grid {
    id: hookActions
    width: parent.width
    columns: 2
    columnSpacing: Style.space(8)
    rowSpacing: Style.space(8)

    readonly property real cellW: (width - columnSpacing) / 2

    Button {
      width: hookActions.cellW
      text: "Save port and URL"
      foreground: root.foreground
      fontFamily: root.fontFamily
      bordered: true
      background: root.actionFill
      enabled: !root.saving
      opacity: enabled ? 1 : 0.4
      onClicked: root.webhookSaveRequested(root.webhookEnabled, root.webhookPortValue(), serveField.text.trim())
    }

    Button {
      width: hookActions.cellW
      text: "Register webhook"
      foreground: root.foreground
      fontFamily: root.fontFamily
      bordered: true
      background: root.actionFill
      enabled: !root.saving && root.session === "live" && root.webhookEnabled
      opacity: enabled ? 1 : 0.4
      tooltipText: root.session === "live" ? "Create the webhook on BlueBubbles" : "Connect to BlueBubbles first"
      onClicked: root.webhookRegisterRequested()
    }

    Button {
      width: hookActions.cellW
      text: "Copy URL"
      foreground: root.foreground
      fontFamily: root.fontFamily
      bordered: true
      background: root.actionFill
      enabled: !root.saving
      opacity: enabled ? 1 : 0.4
      onClicked: root.webhookCopyRequested()
    }

    Button {
      width: hookActions.cellW
      text: "Rotate token"
      foreground: root.foreground
      fontFamily: root.fontFamily
      bordered: true
      background: root.actionFill
      enabled: !root.saving && root.webhookEnabled
      opacity: enabled ? 1 : 0.4
      onClicked: root.webhookRotateRequested()
    }
  }

  TextEdit {
    width: parent.width
    visible: root.webhookCopyUrl.length > 0
    text: root.webhookCopyUrl
    color: root.dim
    readOnly: true
    selectByMouse: true
    wrapMode: TextEdit.Wrap
    font.family: root.fontFamily
    font.pixelSize: Style.font.caption
  }
}
