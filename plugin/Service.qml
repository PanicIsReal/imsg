import QtQuick
import Quickshell
import Quickshell.Io
import "js/ImsgClient.js" as ImsgClient

QtObject {
  id: root
  property int unreadCount: 0
  property var chats: []
  property int openChatId: 0
  property var messages: []
  property bool syncing: true
  property string lastError: ""

  function refreshChats() {
    chatsProc.command = ["sh", "-c", ImsgClient.request("chats.list", { limit: 50 })]
    chatsProc.running = true
  }

  function loadMessages(chatId, before) {
    if (!chatId) return
    var params = { chat_id: chatId, limit: 50 }
    if (before) params.before = before
    historyProc.command = ["sh", "-c", ImsgClient.request("messages.history", params)]
    historyProc.running = true
  }

  function notifyInbound(sender, body, chatId) {
    var safeBody = (body || "").replace(/'/g, "")
    var safeSender = (sender || "iMessage").replace(/'/g, "")
  }

  Timer {
    interval: 5000
    running: true
    repeat: true
    onTriggered: root.refreshChats()
  }

  Process {
    id: chatsProc
    running: false
    stdout: StdioCollector {
      onStreamFinished: {
        var res = ImsgClient.parseResponse(text)
        if (res && res.ok && res.result && res.result.chats) {
          root.chats = res.result.chats
          root.syncing = false
          var total = 0
          for (var i = 0; i < root.chats.length; i++) {
            total += root.chats[i].unread_count || 0
          }
          root.unreadCount = total
        }
      }
    }
    stderr: StdioCollector {
      onStreamFinished: {
        if (text.length > 0) root.lastError = text
      }
    }
  }

  Process {
    id: historyProc
    running: false
    property string beforeCursor: ""
    stdout: StdioCollector {
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

  Component.onCompleted: refreshChats()
}
