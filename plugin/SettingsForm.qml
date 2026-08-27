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
}
