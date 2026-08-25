import QtQuick
import Quickshell
import qs.Commons
import qs.Ui
import "js/Models.js" as Models

Panel {
  id: root
  moduleName: "io.github.panic.imessage"
  manageIpc: true

  property var anchorItem: null
  property var hostWidget: null
  property var imsg: null
  property int selectedChatId: 0

  function open() {
    root.controller.show()
    if (imsg) imsg.refreshChats()
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
    if (imsg) {
      imsg.openChatId = chatId
      imsg.loadMessages(chatId, null)
    }
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
    owner: root.hostWidget || root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(480))
    contentHeight: panel.fittedContentHeight(content.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onCloseRequested: root.close()

      Row {
        id: content
        spacing: Style.space(8)
        width: parent.width
        height: parent.height

        Rectangle {
          width: parent.width * 0.35
          height: parent.height
          color: "transparent"
          border.color: root.barForeground
          opacity: 0.2

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
                  text: modelData.last_message_at || ""
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

        Rectangle {
          width: parent.width * 0.65 - Style.space(8)
          height: parent.height
          color: "transparent"

          Column {
            anchors.fill: parent
            spacing: Style.space(4)

            Text {
              width: parent.width
              visible: imsg && imsg.syncing
              text: "Syncing…"
              color: root.barForeground
              opacity: 0.7
              font.pixelSize: Style.font.caption
            }

            ListView {
              id: threadView
              width: parent.width
              height: parent.height - loadOlderBtn.height - Style.space(4)
              model: imsg ? imsg.messages : []
              clip: true
              spacing: Style.space(4)

              onCountChanged: if (count > 0) positionViewAtEnd()

              delegate: Row {
                width: threadView.width
                layoutDirection: modelData.is_from_me ? Qt.RightToLeft : Qt.LeftToRight
                spacing: Style.space(4)

                Rectangle {
                  width: Math.min(threadView.width * 0.75, bubbleText.implicitWidth + Style.space(16))
                  height: bubbleText.implicitHeight + Style.space(12)
                  radius: 8
                  color: modelData.is_from_me ? Qt.rgba(0.2, 0.5, 1, 0.35) : Qt.rgba(1, 1, 1, 0.1)

                  Text {
                    id: bubbleText
                    anchors.centerIn: parent
                    width: Math.min(threadView.width * 0.7, implicitWidth)
                    wrapMode: Text.WordWrap
                    text: modelData.text || ""
                    color: root.barForeground
                    font.pixelSize: Style.font.body
                  }
                }
              }
            }

            WidgetButton {
              id: loadOlderBtn
              width: parent.width
              visible: selectedChatId > 0 && imsg && imsg.messages.length > 0
              bar: root.bar
              text: "Load older messages"
              onPressed: function() {
                if (!imsg || imsg.messages.length === 0) return
                var oldest = imsg.messages[0]
                if (oldest && oldest.created_at) {
                  imsg.loadMessages(selectedChatId, oldest.created_at)
                }
              }
            }
          }
        }
      }
    }
  }
}
