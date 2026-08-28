import QtQuick
import qs.Commons
import qs.Ui
import "js/Store.js" as Store

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
  property bool advancedOpen: false
  property bool serveOffered: false
  property bool serveActive: false

  readonly property bool editing: urlField.activeFocus || passwordField.activeFocus
  readonly property color dim: Qt.darker(foreground, 1.4)
  readonly property color actionFill: Qt.rgba(0, 0, 0, 0.35)
  readonly property var webhookGuide: Store.webhookGuide({
    enabled: root.webhookEnabled,
    listening: root.webhookListening,
    registered: root.webhookRegistered,
    session: root.session,
    serveOffered: root.serveOffered
  })
  readonly property bool webhookReady: root.webhookGuide.phase === "ready"
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
  signal webhookServeResetRequested()

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
    visible: !root.webhookReady
    text: "Step " + root.webhookGuide.step + " of " + root.webhookGuide.steps
    color: root.dim
    font.family: root.fontFamily
    font.pixelSize: Style.font.caption
  }

  Text {
    width: parent.width
    text: root.webhookGuide.title
    color: root.webhookReady ? root.dim : root.foreground
    font.family: root.fontFamily
    font.pixelSize: root.webhookReady ? Style.font.caption : Style.font.body
    font.bold: !root.webhookReady
    wrapMode: Text.WordWrap
  }

  Text {
    width: parent.width
    visible: root.webhookGuide.body.length > 0
    text: root.webhookGuide.body
    color: root.dim
    font.family: root.fontFamily
    font.pixelSize: Style.font.caption
    wrapMode: Text.WordWrap
  }

  TextField {
    id: portField
    width: parent.width
    visible: !root.webhookReady || root.advancedOpen
    foreground: root.foreground
    text: String(root.webhookPort)
    placeholderText: "Port (default 18792)"
    enabled: !root.saving
  }

  TextField {
    id: serveField
    width: parent.width
    visible: !root.webhookReady || root.advancedOpen
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
  onWebhookEnabledChanged: {
    if (!root.webhookEnabled) root.serveOffered = false
  }

  function webhookPortValue() {
    var port = parseInt(portField.text, 10)
    if (!port || port < 1 || port > 65535) port = 18792
    return port
  }

  function persistWebhook(enabled) {
    root.webhookSaveRequested(!!enabled, root.webhookPortValue(), serveField.text.trim())
  }

  function runWebhookStep() {
    if (root.saving) return
    var kind = root.webhookGuide.actionKind
    if (kind === "enable") {
      root.persistWebhook(true)
      return
    }
    if (kind === "serve") {
      var port = root.webhookPortValue()
      root.persistWebhook(true)
      root.webhookServeRequested(port)
      root.serveOffered = true
      return
    }
    if (kind === "register") {
      root.webhookRegisterRequested()
      return
    }
    if (kind === "reconnect") root.reconnectRequested()
  }

  Button {
    width: parent.width
    visible: root.webhookGuide.actionKind !== ""
    text: root.saving ? "Working…" : root.webhookGuide.actionLabel
    foreground: root.foreground
    fontFamily: root.fontFamily
    bordered: true
    background: root.actionFill
    enabled: !root.saving
    opacity: enabled ? 1 : 0.4
    onClicked: root.runWebhookStep()
  }

  Button {
    width: parent.width
    visible: root.serveActive
    text: "Remove serve"
    foreground: root.foreground
    fontFamily: root.fontFamily
    bordered: true
    background: root.actionFill
    enabled: !root.saving
    opacity: enabled ? 1 : 0.4
    onClicked: {
      root.webhookServeResetRequested()
      root.serveOffered = false
    }
  }

  Toggle {
    width: parent.width
    visible: root.webhookReady
    label: "Toggle"
    description: "Webhook on. Poll is off."
    checked: root.webhookEnabled
    foreground: root.foreground
    fontFamily: root.fontFamily
    opacity: root.saving ? 0.55 : 1
    onClicked: {
      if (root.saving) return
      root.persistWebhook(!root.webhookEnabled)
    }
  }

  Button {
    width: parent.width
    visible: root.webhookEnabled && !root.webhookReady
    text: "Turn off"
    foreground: root.foreground
    fontFamily: root.fontFamily
    bordered: true
    enabled: !root.saving
    onClicked: root.persistWebhook(false)
  }

  Button {
    width: parent.width
    text: root.advancedOpen ? "Hide advanced" : "Advanced"
    foreground: root.foreground
    fontFamily: root.fontFamily
    bordered: true
    onClicked: root.advancedOpen = !root.advancedOpen
  }

  Grid {
    id: hookActions
    width: parent.width
    visible: root.advancedOpen
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
      onClicked: root.persistWebhook(root.webhookEnabled)
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

    Button {
      width: hookActions.cellW
      text: "Publish with Tailscale"
      foreground: root.foreground
      fontFamily: root.fontFamily
      bordered: true
      background: root.actionFill
      enabled: !root.saving
      opacity: enabled ? 1 : 0.4
      onClicked: {
        var port = root.webhookPortValue()
        root.persistWebhook(root.webhookEnabled)
        root.webhookServeRequested(port)
        root.serveOffered = true
      }
    }
  }

  TextEdit {
    width: parent.width
    visible: root.advancedOpen && root.webhookCopyUrl.length > 0
    text: root.webhookCopyUrl
    color: root.dim
    readOnly: true
    selectByMouse: true
    wrapMode: TextEdit.Wrap
    font.family: root.fontFamily
    font.pixelSize: Style.font.caption
  }
}
