import QtQuick
import Quickshell
import qs.Commons
import qs.Ui
import "js/Models.js" as Models

Panel {
  id: root
  moduleName: "io.github.panic.imessage"
  ipcTarget: "io.github.panic.imessage"
  manageIpc: false

  property var anchorItem: null
  property var hostWidget: null
  property var imsg: null
  property int selectedChatId: 0
  property string draftText: ""

  readonly property var barIdentity: hostWidget || root
  readonly property color dim: Qt.rgba(root.barForeground.r, root.barForeground.g, root.barForeground.b, 0.15)
  readonly property string statusLine: {
    if (!imsg) return ""
    if (imsg.lastError && imsg.lastError.length > 0) return imsg.lastError
    if (!imsg.connected) return "Connecting to imsg-sync…"
    if (imsg.bridgeConnected && !imsg.databaseReady) return "Mac is online. Messages database is locked (grant Full Disk Access to imsg on the Mac)."
    if (!imsg.bridgeConnected) return "Showing cached messages. Mac link is down."
    return ""
  }

  function open() {
    root.controller.show()
    if (imsg) {
      imsg.refreshChats()
      imsg.refreshStatus()
    }
  }

  function close() {
    root.controller.hide()
  }

  function toggle() {
    if (opened) close()
    else open()
  }

  function openChat(chatId) {
    selectedChatId = chatId
    draftText = ""
    if (imsg) {
      imsg.openChatId = chatId
      imsg.loadMessages(chatId, null)
    }
  }

  function sendDraft() {
    if (!imsg || selectedChatId <= 0 || draftText.trim().length === 0) return
    imsg.sendMessage(selectedChatId, draftText)
    draftText = ""
  }

  function call(method, args) {
    if (method === "openChat" && args && args.chat_id) {
      open()
      openChat(args.chat_id)
      return "ok"
    }
    return "unknown"
  }

  KeyboardPanel {
    id: panel
    anchorItem: root.anchorItem
    owner: root.barIdentity
    bar: root.bar
    open: root.opened
    centerOnBar: true
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(560))
    contentHeight: panel.fittedContentHeight(Style.space(420))

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      blocked: draftField.activeFocus
      onCloseRequested: root.close()

      Item {
        id: content
        anchors.fill: parent
        clip: true

        Item {
          id: chatPane
          anchors.left: parent.left
          anchors.top: parent.top
          anchors.bottom: parent.bottom
          width: Math.floor(parent.width * 0.34)

          Rectangle {
            anchors.fill: parent
            color: "transparent"
            border.color: root.dim
          }

          ListView {
            id: chatList
            anchors.fill: parent
            anchors.margins: Style.space(4)
            model: imsg ? imsg.chats : []
            clip: true
            delegate: Rectangle {
              width: chatList.width
              height: chatRow.implicitHeight + Style.space(8)
              color: selectedChatId === modelData.id ? Qt.rgba(1, 1, 1, 0.08) : "transparent"
              radius: 4

              Column {
                id: chatRow
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                anchors.margins: Style.space(4)
                spacing: Style.space(2)

                Text {
                  width: parent.width
                  text: Models.chatTitle(modelData)
                  color: root.barForeground
                  font.bold: true
                  elide: Text.ElideRight
                }
                Text {
                  width: parent.width
                  text: Models.formatTime(modelData.last_message_at)
                  color: root.barForeground
                  opacity: 0.6
                  font.pixelSize: Style.font.caption
                }
              }

              MouseArea {
                anchors.fill: parent
                onClicked: root.openChat(modelData.id)
              }
            }
          }
        }

        Item {
          id: threadPane
          anchors.left: chatPane.right
          anchors.right: parent.right
          anchors.top: parent.top
          anchors.bottom: parent.bottom
          anchors.leftMargin: Style.space(8)

          Item {
            id: statusSlot
            anchors.top: parent.top
            width: parent.width
            height: root.statusLine.length > 0 ? statusText.implicitHeight : 0
            clip: true

            Text {
              id: statusText
              width: parent.width
              text: root.statusLine
              color: imsg && imsg.lastError && imsg.lastError.length > 0 ? "#ff6b6b" : root.barForeground
              opacity: 0.8
              wrapMode: Text.WordWrap
              font.pixelSize: Style.font.caption
            }
          }

          ListView {
            id: threadView
            anchors.top: statusSlot.bottom
            anchors.topMargin: statusSlot.height > 0 ? Style.space(4) : 0
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: composerRow.top
            anchors.bottomMargin: Style.space(4)
            model: imsg ? imsg.messages : []
            clip: true
            spacing: Style.space(4)
            boundsBehavior: Flickable.StopAtBounds

            onCountChanged: if (count > 0) positionViewAtEnd()

            header: Item {
              width: threadView.width
              height: selectedChatId > 0 && imsg && imsg.messages && imsg.messages.length > 0 ? Style.space(28) : 0
              WidgetButton {
                anchors.fill: parent
                visible: parent.height > 0
                bar: root.bar
                text: "Load older"
                onPressed: function() {
                  if (!imsg || imsg.messages.length === 0) return
                  var oldest = imsg.messages[0]
                  if (oldest && oldest.created_at) imsg.loadMessages(selectedChatId, oldest.created_at)
                }
              }
            }

            delegate: Item {
              width: threadView.width
              height: bubble.implicitHeight + Style.space(4)

              Rectangle {
                id: bubble
                anchors.left: modelData.is_from_me ? undefined : parent.left
                anchors.right: modelData.is_from_me ? parent.right : undefined
                width: Math.min(threadView.width * 0.78, bubbleText.implicitWidth + Style.space(16))
                implicitHeight: bubbleText.implicitHeight + Style.space(12)
                radius: 8
                color: modelData.is_from_me ? Qt.rgba(0.2, 0.5, 1, 0.35) : Qt.rgba(1, 1, 1, 0.1)

                Text {
                  id: bubbleText
                  anchors.centerIn: parent
                  width: Math.min(threadView.width * 0.72, implicitWidth)
                  wrapMode: Text.Wrap
                  text: modelData.text || ""
                  color: root.barForeground
                  font.pixelSize: Style.font.body
                }
              }
            }
          }

          Row {
            id: composerRow
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            height: Style.space(36)
            spacing: Style.space(4)

            Rectangle {
              width: parent.width - sendBtn.width - Style.space(4)
              height: parent.height
              radius: 8
              color: Qt.rgba(1, 1, 1, 0.06)
              border.width: 1
              border.color: root.dim

              TextInput {
                id: draftField
                anchors.fill: parent
                anchors.margins: Style.space(8)
                text: root.draftText
                onTextChanged: root.draftText = text
                color: root.barForeground
                font.pixelSize: Style.font.body
                clip: true
                selectByMouse: true
                enabled: selectedChatId > 0 && imsg && !imsg.sending
                Keys.onReturnPressed: function(event) {
                  if (!(event.modifiers & Qt.ShiftModifier)) {
                    event.accepted = true
                    root.sendDraft()
                  }
                }
              }
            }

            WidgetButton {
              id: sendBtn
              bar: root.bar
              text: imsg && imsg.sending ? "…" : "Send"
              enabled: selectedChatId > 0 && imsg && !imsg.sending && root.draftText.trim().length > 0
              onPressed: function() { root.sendDraft() }
            }
          }
        }
      }
    }
  }
}
