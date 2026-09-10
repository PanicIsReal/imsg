import QtQuick
import Quickshell
import Quickshell.Io
import "js/ImsgClient.js" as ImsgClient
import "js/Store.js" as Store

Item {
  id: root
  signal frameMetricRequested(string metric, double startedAt)
  signal databaseReset()
  property int unreadCount: 0
  property var chats: []
  property int openChatId: 0
  property var messages: []
  property var messagesByChat: ({})
  property var outbox: []
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
  property var queuedHistory: null
  property int chatGeneration: 0
  property int clientSequence: 0
  property var messageRevisions: ({})
  property int cacheGeneration: 0
  property string databaseGeneration: ""
  readonly property bool realtimeMetrics: Quickshell.env("IMSG_REALTIME_METRICS") === "1"

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
    var eventStartedAt = Date.now()
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
      if (frame.topic === "sync.message" && frame.payload && frame.payload.message) {
        root.bumpMessageRevision(frame.payload.message.chat_id)
      }
      applyPatch(Store.applyEvent({
        chats: root.chats,
        messagesByChat: root.messagesByChat,
        outbox: root.outbox,
        openChatId: root.openChatId
      }, frame))
      root.recordWallMetric("event_to_model_wall", eventStartedAt)
      if (root.realtimeMetrics && frame.topic === "sync.message" && frame.payload && frame.payload.message && Number(frame.payload.message.chat_id) === root.openChatId) {
        root.frameMetricRequested("event_to_next_frame_wall", eventStartedAt)
      }
    }
  }

  function applyPatch(patch) {
    if (!patch) return
    if (patch.chats !== undefined) root.chats = patch.chats
    if (patch.messages !== undefined) root.messages = patch.messages
    if (patch.messagesByChat !== undefined) root.messagesByChat = patch.messagesByChat
    if (patch.outbox !== undefined) root.outbox = patch.outbox
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
    var generation = patch.databaseGeneration || (patch.link && patch.link.db_generation) || ""
    var resetGeneration = Store.shouldResetGeneration(root.databaseGeneration, generation, patch.resetGeneration)
    if (generation !== "") root.databaseGeneration = generation
    if (resetGeneration) {
      root.messagesByChat = ({})
      root.outbox = Store.resetOutboxGeneration(root.outbox)
      root.messageRevisions = ({})
      root.cacheGeneration += 1
      root.databaseReset()
    }
    root.updateVisibleMessages()
    if (patch.resyncOpenChat) {
      root.refreshChats()
      if (root.openChatId > 0) root.loadMessages(root.openChatId, null)
    }
  }

  function updateVisibleMessages() {
    root.messages = Store.messagesForChat(root.messagesByChat, root.outbox, root.openChatId)
  }

  function recordWallMetric(metric, startedAt) {
    if (!root.realtimeMetrics) return
    console.log(JSON.stringify({ metric: metric, elapsed_ms: Math.max(0, Date.now() - startedAt), clock: "wall" }))
  }

  function messageRevision(chatId) {
    return root.messageRevisions[String(Number(chatId))] || 0
  }

  function bumpMessageRevision(chatId) {
    var key = String(Number(chatId))
    var copy = {}
    for (var existing in root.messageRevisions) copy[existing] = root.messageRevisions[existing]
    copy[key] = (copy[key] || 0) + 1
    root.messageRevisions = copy
  }

  function nextClientId() {
    root.clientSequence += 1
    return "qml-" + Date.now() + "-" + root.clientSequence
  }

  onOpenChatIdChanged: {
    root.chatGeneration += 1
    root.updateVisibleMessages()
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
    if (!chatId || requestScript === "") return
    var request = {
      chatId: chatId,
      before: before || "",
      generation: root.chatGeneration,
      messageRevision: root.messageRevision(chatId),
      cacheGeneration: root.cacheGeneration
    }
    if (historyProc.running) {
      root.queuedHistory = request
      return
    }
    root.startHistory(request)
  }

  function startHistory(request) {
    var chatId = request.chatId
    var params = { chat_id: chatId, limit: 50 }
    if (request.before) params.before = request.before
    historyProc.requestChatId = chatId
    historyProc.requestGeneration = request.generation
    historyProc.messageRevision = request.messageRevision
    historyProc.cacheGeneration = request.cacheGeneration
    historyProc.beforeCursor = request.before
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
    var sendStartedAt = Date.now()
    var body = String(text || "").trim()
    if (!chatId || body.length === 0 || requestScript === "") return false
    var clientId = root.nextClientId()
    root.outbox = Store.enqueue(root.outbox, chatId, body, clientId, new Date().toISOString())
    root.updateVisibleMessages()
    root.recordWallMetric("send_to_model_wall", sendStartedAt)
    if (root.realtimeMetrics) root.frameMetricRequested("send_to_next_frame_wall", sendStartedAt)
    root.drainOutbox()
    return true
  }

  function retryMessage(clientId) {
    root.outbox = Store.retryAs(root.outbox, clientId, root.nextClientId(), new Date().toISOString())
    root.updateVisibleMessages()
    root.drainOutbox()
  }

  function drainOutbox() {
    if (sendProc.running || requestScript === "") return
    var entry = Store.nextQueued(root.outbox)
    if (!entry) { root.sending = false; return }
    root.sendError = ""
    root.outbox = Store.markOutbox(root.outbox, entry.client_id, "sending", "", null)
    root.updateVisibleMessages()
    sendProc.clientId = entry.client_id
    sendProc.chatId = entry.chat_id
    sendProc.cacheGeneration = root.cacheGeneration
    sendProc.response = null
    sendProc.command = ImsgClient.command(requestScript, "messages.send", { chat_id: entry.chat_id, text: entry.text, client_id: entry.client_id })
    sendProc.running = true
    root.sending = true
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
    property int requestChatId: 0
    property int requestGeneration: 0
    property int messageRevision: 0
    property int cacheGeneration: 0
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var res = ImsgClient.parseResponse(text)
        if (historyProc.cacheGeneration === root.cacheGeneration && res && res.ok && res.result && res.result.messages) {
          var unchanged = historyProc.messageRevision === root.messageRevision(historyProc.requestChatId)
          root.messagesByChat = Store.mergeHistory(root.messagesByChat, historyProc.requestChatId, res.result.messages, unchanged)
          if (historyProc.requestGeneration === root.chatGeneration && Number(historyProc.requestChatId) === Number(root.openChatId)) root.updateVisibleMessages()
        }
      }
    }
    onExited: function() {
      historyProc.beforeCursor = ""
      if (root.queuedHistory) {
        var next = root.queuedHistory
        root.queuedHistory = null
        root.startHistory(next)
      }
    }
  }

  Process {
    id: sendProc
    running: false
    command: []
    property int chatId: 0
    property string clientId: ""
    property int cacheGeneration: 0
    property var response: null
    property string stderrText: ""
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var res = ImsgClient.parseResponse(text)
        if (res && res.ok) {
          sendProc.response = res
        } else if (res && res.error) {
          sendProc.response = res
        }
      }
    }
    stderr: StdioCollector {
      waitForEnd: true
      onStreamFinished: { sendProc.stderrText = text }
    }
    onExited: function(exitCode) {
      var res = sendProc.response
      if (sendProc.cacheGeneration !== root.cacheGeneration) {
        root.outbox = Store.failOutbox(root.outbox, sendProc.clientId, "unconfirmed", "Message database changed before confirmation")
      } else if (exitCode === 0 && res && res.ok) {
        root.outbox = Store.acknowledge(root.outbox, sendProc.clientId, res.result || {})
        root.refreshChats()
      } else if (res && res.error) {
        var error = ImsgClient.friendlyError(res.error.message || "send failed")
        var failureStatus = Store.classifySendFailure(res.error)
        root.outbox = Store.failOutbox(root.outbox, sendProc.clientId, failureStatus, error)
        if (Store.outboxStatus(root.outbox, sendProc.clientId) === failureStatus) root.sendError = error
      } else {
        var uncertain = ImsgClient.friendlyError(sendProc.stderrText.trim() || ("send ended without confirmation (code " + exitCode + ")"))
        root.outbox = Store.failOutbox(root.outbox, sendProc.clientId, "unconfirmed", uncertain)
        if (Store.outboxStatus(root.outbox, sendProc.clientId) === "unconfirmed") root.sendError = uncertain
      }
      root.updateVisibleMessages()
      sendProc.stderrText = ""
      sendProc.response = null
      sendProc.clientId = ""
      root.sending = false
      root.drainOutbox()
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
    onExited: function() {
      if (root.openChatId > 0) root.loadMessages(root.openChatId, null)
      streamRetry.restart()
    }
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
