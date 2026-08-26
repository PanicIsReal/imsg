import QtQuick
import Quickshell
import Quickshell.Io
import qs.Ui

BarWidget {
  id: root
  moduleName: "io.github.panic.imessage"

  property var shellService: null
  readonly property var imsg: shellService
  readonly property bool opened: panelLoader.item
    ? panelLoader.item.opened === true
    : false
  readonly property bool cacheReady: imsg && imsg.cacheReady
  readonly property string linkState: imsg ? imsg.linkState : "waiting"

  function open() {
    if (panelLoader.item) panelLoader.item.open()
  }

  function close() {
    if (panelLoader.item) panelLoader.item.close()
  }

  function toggle() {
    if (panelLoader.item) panelLoader.item.toggle()
  }

  function injectPanel() {
    if (!panelLoader.item) return
    panelLoader.item.bar = root.bar
    panelLoader.item.anchorItem = button
    panelLoader.item.hostWidget = root
    panelLoader.item.imsg = imsg
  }

  function resolveService() {
    if (!bar || !bar.shell) return
    var svc = bar.shell.serviceFor("io.github.panic.imessage")
    if (svc && svc !== shellService) {
      shellService = svc
      injectPanel()
    }
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  onBarChanged: {
    resolveService()
    if (!shellService) servicePoll.restart()
  }

  Timer {
    id: servicePoll
    interval: 500
    repeat: true
    onTriggered: {
      root.resolveService()
      if (root.shellService) stop()
    }
  }

  Loader {
    id: panelLoader
    active: true
    source: Qt.resolvedUrl("Panel.qml")
    visible: false
    onLoaded: {
      root.injectPanel()
      Qt.callLater(root.injectPanel)
    }
    onStatusChanged: {
      if (status === Loader.Error) console.warn("imessage panel failed:", errorString())
    }
  }

  IpcHandler {
    target: "io.github.panic.imessage"

    function open(): void { root.open() }
    function close(): void { root.close() }
    function show(): void { root.open() }
    function hide(): void { root.close() }
    function toggle(): void { root.toggle() }
  }

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    dimmed: root.linkState === "mac-down" || root.linkState === "sync-down" || root.linkState === "mac-locked"
    text: imsg && imsg.unreadCount > 0 ? "󰍩 " + imsg.unreadCount : "󰍩"
    tooltipText: {
      var guide = imsg && imsg.setupGuide
      if (guide && guide.phase === "ready") {
        return imsg && imsg.unreadCount > 0 ? "iMessage · " + imsg.unreadCount + " unread" : "iMessage"
      }
      if (guide && guide.title) return guide.title
      return "iMessage"
    }
    onPressed: function(buttonCode) {
      if (buttonCode === Qt.LeftButton) root.toggle()
    }
  }
}
