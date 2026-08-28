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
      : "Off. Live mail uses a 2s poll."
    color: root.dim
    font.family: root.fontFamily
    font.pixelSize: Style.font.caption
    wrapMode: Text.WordWrap
  }

  Button {
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

  Row {
    width: parent.width
    spacing: Style.space(8)

    Button {
      text: root.webhookEnabled ? "Disable webhook" : "Enable webhook"
      foreground: root.foreground
      fontFamily: root.fontFamily
      enabled: !root.saving
      onClicked: {
        var port = parseInt(portField.text, 10)
        if (!port) port = 18792
        root.webhookSaveRequested(!root.webhookEnabled, port, serveField.text.trim())
      }
    }

    Button {
      text: "Save port"
      foreground: root.foreground
      fontFamily: root.fontFamily
      bordered: true
      enabled: !root.saving
      onClicked: {
        var port = parseInt(portField.text, 10)
        if (!port) port = 18792
        root.webhookSaveRequested(root.webhookEnabled, port, serveField.text.trim())
      }
    }
  }

  Row {
    width: parent.width
    spacing: Style.space(8)

    Button {
      text: "Register webhook"
      foreground: root.foreground
      fontFamily: root.fontFamily
      enabled: !root.saving && root.session === "live" && root.webhookEnabled
      tooltipText: root.session === "live" ? "Create the webhook on BlueBubbles" : "Connect to BlueBubbles first"
      onClicked: root.webhookRegisterRequested()
    }

    Button {
      text: "Copy URL"
      foreground: root.foreground
      fontFamily: root.fontFamily
      bordered: true
      enabled: !root.saving
      onClicked: root.webhookCopyRequested()
    }

    Button {
      text: "Rotate token"
      foreground: root.foreground
      fontFamily: root.fontFamily
      bordered: true
      enabled: !root.saving && root.webhookEnabled
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
