import QtQuick
import Quickshell
import Quickshell.Io
import "js/ImsgClient.js" as ImsgClient
import "js/Store.js" as Store

Item {
  id: root
  property int unreadCount: 0
  property var chats: []
  property int openChatId: 0
  property var messages: []
  property bool syncing: true
  property bool connected: false
  property bool bridgeConnected: false
  property bool databaseReady: false
  property bool sending: false
  property bool statusKnown: false
  property string lastError: ""
  property string sendError: ""
  property string contacts: "unknown"
  property var pendingNotify: null

  readonly property bool cacheReady: chats && chats.length > 0
  readonly property string linkState: {
    if (!connected && !cacheReady) return "waiting"
    if (!connected) return "sync-down"
    if (!statusKnown) return "checking"
    if (bridgeConnected && databaseReady) return "live"
    if (bridgeConnected) return "mac-locked"
    return "mac-down"
  }
  readonly property string contactsState: root.contacts || "unknown"
  readonly property var setupGuide: Store.setupGuide({
    connected: root.connected,
    cacheReady: root.cacheReady,
    statusKnown: root.statusKnown,
    bridgeConnected: root.bridgeConnected,
    databaseReady: root.databaseReady,
    lastError: root.lastError,
    contacts: root.contacts
  })
  readonly property string requestScript: {
    var resolved = ImsgClient.scriptPath(Qt.resolvedUrl("bin/request.py"))
    if (resolved !== "") return resolved
    var home = Quickshell.env("HOME") || ""
    return home + "/.config/omarchy/plugins/io.github.panic.imessage/bin/request.py"
  }
  readonly property string subscribeScript: {
    var resolved = ImsgClient.scriptPath(Qt.resolvedUrl("bin/subscribe.py"))
    if (resolved !== "") return resolved
    var home = Quickshell.env("HOME") || ""
    return home + "/.config/omarchy/plugins/io.github.panic.imessage/bin/subscribe.py"
  }

  function ingest(line) {
    var frame = ImsgClient.parseResponse(line)
    if (!frame) return
    if (frame.type === "res" && frame.ok && frame.result) {
      applyPatch(Store.applySnapshot(frame.result))
      root.connected = true
      root.syncing = false
      if (root.openChatId > 0) root.loadMessages(root.openChatId, null)
      return
    }
    if (frame.type === "event") {
      applyPatch(Store.applyEvent({
        chats: root.chats,
        messages: root.messages,
        openChatId: root.openChatId
      }, frame))
    }
  }

  function applyPatch(patch) {
    if (!patch) return
    if (patch.chats !== undefined) root.chats = patch.chats
    if (patch.messages !== undefined) root.messages = patch.messages
    if (patch.unreadCount !== undefined) root.unreadCount = patch.unreadCount
    if (patch.link !== undefined) {
      root.bridgeConnected = ImsgClient.flag(patch.link.bridge_connected)
      root.databaseReady = ImsgClient.flag(patch.link.database_ready)
      root.lastError = ImsgClient.friendlyError(patch.link.last_error)
      root.statusKnown = true
      if (patch.link.contacts !== undefined && patch.link.contacts !== null && String(patch.link.contacts).length > 0) {
        root.contacts = String(patch.link.contacts)
      }
    }
    if (patch.notify) {
      root.notifyInbound(patch.notify.sender, patch.notify.preview, patch.notify.chatId)
    }
  }

  function refreshChats() {
    if (requestScript === "" || chatsProc.running) return
    chatsProc.command = ImsgClient.command(requestScript, "chats.list", { limit: 50 })
    chatsProc.running = true
  }

  function refreshStatus() {
    if (requestScript === "" || statusProc.running) return
    statusProc.command = ImsgClient.command(requestScript, "status", {})
    statusProc.running = true
  }

  function loadMessages(chatId, before) {
    if (!chatId || requestScript === "" || historyProc.running) return
    var params = { chat_id: chatId, limit: 50 }
    if (before) params.before = before
    historyProc.beforeCursor = before || ""
    historyProc.command = ImsgClient.command(requestScript, "messages.history", params)
    historyProc.running = true
  }

  function requestContactsAccess() {
    if (requestScript === "" || contactsProc.running) return
    root.contacts = "prompting"
    contactsProc.command = ImsgClient.command(requestScript, "contacts.authorize", {})
    contactsProc.running = true
  }

  function sendMessage(chatId, text) {
    if (!chatId || !text || text.trim().length === 0 || requestScript === "" || sendProc.running) return
    sendError = ""
    sendProc.chatId = chatId
    sendProc.command = ImsgClient.command(requestScript, "messages.send", {
      chat_id: chatId,
      text: text.trim()
    })
    sendProc.running = true
    sending = true
  }

  function notifyInbound(sender, body, chatId) {
    var cmd = ImsgClient.notificationCommand(sender, body, chatId)
    if (notifyProc.running) {
      root.pendingNotify = cmd
      return
    }
    notifyProc.command = cmd
    notifyProc.running = true
  }

  Timer {
    interval: 5000
    running: true
    repeat: true
    onTriggered: {
      root.refreshChats()
      root.refreshStatus()
    }
  }

  Process {
    id: chatsProc
    running: false
    command: []
    property string stderrText: ""
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var res = ImsgClient.parseResponse(text)
        if (res && res.ok && res.result && res.result.chats) {
          root.chats = res.result.chats
          root.syncing = false
          root.connected = true
          var total = 0
          for (var i = 0; i < root.chats.length; i++) {
            total += root.chats[i].unread_count || 0
          }
          root.unreadCount = total
        } else if (res && !res.ok) {
          root.connected = false
        } else if (!res) {
          root.connected = false
        }
      }
    }
    stderr: StdioCollector {
      waitForEnd: true
      onStreamFinished: { chatsProc.stderrText = text }
    }
    onExited: function(exitCode) {
      if (exitCode !== 0) {
        root.connected = false
      }
      chatsProc.stderrText = ""
    }
  }

  Process {
    id: statusProc
    running: false
    command: []
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var res = ImsgClient.parseResponse(text)
        if (res && res.ok && res.result) {
          root.bridgeConnected = ImsgClient.flag(res.result.bridge_connected)
          root.databaseReady = ImsgClient.flag(res.result.database_ready)
          root.lastError = ImsgClient.friendlyError(res.result.last_error)
          root.statusKnown = true
          if (res.result.contacts) root.contacts = String(res.result.contacts)
        }
      }
    }
  }

  Process {
    id: contactsProc
    running: false
    command: []
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var res = ImsgClient.parseResponse(text)
        if (!res) return
        if (res.ok && res.result && res.result.outcome === "granted" && res.result.names_visible) {
          root.contacts = "granted"
        } else if (res.ok && res.result && res.result.outcome === "prompting") {
          root.contacts = "prompting"
        } else if (res.ok) {
          root.contacts = "unavailable"
        }
      }
    }
  }

  Process {
    id: historyProc
    running: false
    command: []
    property string beforeCursor: ""
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var res = ImsgClient.parseResponse(text)
        if (res && res.ok && res.result && res.result.messages) {
          if (historyProc.beforeCursor.length > 0) {
            root.messages = res.result.messages.concat(root.messages)
          } else {
            root.messages = res.result.messages
          }
        }
        historyProc.beforeCursor = ""
      }
    }
  }

  Process {
    id: sendProc
    running: false
    command: []
    property int chatId: 0
    property string stderrText: ""
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var res = ImsgClient.parseResponse(text)
        if (res && res.ok) {
          root.sendError = ""
          root.loadMessages(sendProc.chatId, null)
          root.refreshChats()
        } else if (res && res.error) {
          root.sendError = ImsgClient.friendlyError(res.error.message || "send failed")
        }
      }
    }
    stderr: StdioCollector {
      waitForEnd: true
      onStreamFinished: { sendProc.stderrText = text }
    }
    onExited: function(exitCode) {
      root.sending = false
      if (exitCode !== 0) {
        root.sendError = ImsgClient.friendlyError(sendProc.stderrText.trim() || ("send failed (code " + exitCode + ")"))
      }
      sendProc.stderrText = ""
    }
  }

  Process {
    id: notifyProc
    running: false
    command: []
    onExited: function() {
      if (!root.pendingNotify) return
      notifyProc.command = root.pendingNotify
      root.pendingNotify = null
      notifyProc.running = true
    }
  }

  Process {
    id: streamProc
    running: false
    command: []
    stdout: SplitParser {
      onRead: function(data) { root.ingest(data) }
    }
    onExited: function() { streamRetry.restart() }
  }

  Timer {
    id: streamRetry
    interval: 2000
    repeat: false
    onTriggered: {
      if (root.subscribeScript === "") return
      streamProc.command = ImsgClient.streamCommand(root.subscribeScript)
      streamProc.running = true
    }
  }

  Component.onCompleted: {
    refreshChats()
    refreshStatus()
    if (root.subscribeScript !== "") {
      streamProc.command = ImsgClient.streamCommand(root.subscribeScript)
      streamProc.running = true
    }
  }
}
