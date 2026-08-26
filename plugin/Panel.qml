import QtQuick
import Quickshell
import qs.Commons
import qs.Ui
import "js/Models.js" as Models
import "js/Store.js" as Store

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
  readonly property color wash: Qt.rgba(root.barForeground.r, root.barForeground.g, root.barForeground.b, 0.08)
  readonly property color washStrong: Qt.rgba(root.barForeground.r, root.barForeground.g, root.barForeground.b, 0.14)
  readonly property string currentTitle: {
    if (!imsg || !imsg.chats || selectedChatId <= 0) return ""
    for (var i = 0; i < imsg.chats.length; i++) {
      if (imsg.chats[i].id === selectedChatId) return Models.chatTitle(imsg.chats[i])
    }
    return ""
  }
  readonly property var setupGuide: imsg && imsg.setupGuide ? imsg.setupGuide : Store.setupGuide({
    connected: false,
    cacheReady: false,
    statusKnown: false,
    bridgeConnected: false,
    databaseReady: false,
    lastError: "",
    contacts: "unknown"
  })
  readonly property bool setupReady: root.setupGuide.phase === "ready"
  readonly property string statusLine: {
    if (!imsg) return ""
    if (imsg.sendError && imsg.sendError.length > 0) return imsg.sendError
    if (imsg.linkState === "mac-locked") return "Messages is locked on your Mac. Grant Full Disk Access to Ghostty."
    if (imsg.linkState === "mac-down") return "Showing saved messages. The Mac link is down."
    return ""
  }

  function maybeSelectFirst() {
    if (selectedChatId > 0 || !imsg || !imsg.chats || imsg.chats.length === 0) return
    openChat(imsg.chats[0].id)
  }

  function open() {
    root.controller.show()
    if (imsg) {
      imsg.refreshChats()
      imsg.refreshStatus()
      if (selectedChatId > 0) openChat(selectedChatId)
      else maybeSelectFirst()
    }
  }

  function close() {
    if (imsg) imsg.openChatId = 0
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
    gap: Style.space(16)
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(720))
    contentHeight: panel.cappedContentHeight(Style.space(540))

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      clip: true
      blocked: draftField.activeFocus
      onCloseRequested: root.close()

      Item {
        id: content
        anchors.fill: parent
        clip: true

        Column {
          id: setupCard
          visible: !root.setupReady
          anchors.centerIn: parent
          width: Math.min(parent.width - Style.space(48), Style.space(420))
          spacing: Style.space(12)

          Text {
            width: parent.width
            text: root.setupGuide.title
            color: root.barForeground
            font.bold: true
            wrapMode: Text.WordWrap
            horizontalAlignment: Text.AlignHCenter
          }
          Text {
            width: parent.width
            text: root.setupGuide.body
            color: root.barForeground
            wrapMode: Text.WordWrap
            horizontalAlignment: Text.AlignHCenter
            opacity: 0.85
          }
          TextEdit {
            width: parent.width
            visible: root.setupGuide.hint && root.setupGuide.hint.length > 0
            text: root.setupGuide.hint || ""
            color: root.barForeground
            readOnly: true
            selectByMouse: true
            wrapMode: TextEdit.Wrap
            font.family: "monospace"
            font.pixelSize: Style.font.caption
            horizontalAlignment: TextEdit.AlignHCenter
            opacity: 0.7
          }
        }

        Row {
          id: panes
          visible: root.setupReady
          anchors.fill: parent
          spacing: Style.space(8)

        Item {
          id: chatPane
          width: Math.floor(parent.width * 0.34)
          height: parent.height
          clip: true

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
              color: selectedChatId === modelData.id ? root.washStrong : "transparent"
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
                Row {
                  width: parent.width
                  spacing: Style.space(8)

                  Text {
                    text: Models.formatTime(modelData.last_message_at)
                    color: root.barForeground
                    opacity: 0.6
                    font.pixelSize: Style.font.caption
                  }
                  Text {
                    visible: (modelData.unread_count || 0) > 0
                    text: String(modelData.unread_count || 0)
                    color: root.barForeground
                    opacity: 0.7
                    font.pixelSize: Style.font.caption
                  }
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
          width: parent.width - chatPane.width - parent.spacing
          height: parent.height
          clip: true

          Text {
            id: statusText
            anchors.top: parent.top
            width: parent.width
            height: root.statusLine.length > 0 ? Style.space(36) : 0
            visible: height > 0
            clip: true
            text: root.statusLine
            color: imsg && imsg.sendError && imsg.sendError.length > 0 ? "#ff6b6b" : root.barForeground
            opacity: 0.8
            wrapMode: Text.WordWrap
            maximumLineCount: 2
            elide: Text.ElideRight
            font.pixelSize: Style.font.caption
          }

          Item {
            id: contactsRow
            anchors.top: statusText.bottom
            anchors.topMargin: visible ? Style.space(4) : 0
            width: parent.width
            height: visible ? Style.space(28) : 0
            visible: root.setupGuide.actionKind === "contacts" || (imsg && imsg.contactsState === "prompting")
            clip: true

            WidgetButton {
              anchors.fill: parent
              visible: root.setupGuide.actionKind === "contacts"
              bar: root.bar
              text: "Show contact names…"
              onPressed: function() { imsg.requestContactsAccess() }
            }
            Text {
              anchors.fill: parent
              visible: imsg && imsg.contactsState === "prompting"
              text: "Click Allow on your Mac to show contact names."
              color: root.barForeground
              opacity: 0.8
              wrapMode: Text.WordWrap
              elide: Text.ElideRight
              font.pixelSize: Style.font.caption
              verticalAlignment: Text.AlignVCenter
            }
          }

          Text {
            id: threadTitle
            anchors.top: contactsRow.bottom
            anchors.topMargin: contactsRow.visible || statusText.visible ? Style.space(4) : 0
            width: parent.width
            height: root.currentTitle.length > 0 ? Style.space(22) : 0
            visible: height > 0
            text: root.currentTitle
            color: root.barForeground
            font.bold: true
            elide: Text.ElideRight
          }

          ListView {
            id: threadView
            anchors.top: threadTitle.bottom
            anchors.topMargin: threadTitle.visible ? Style.space(4) : 0
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: composerRow.top
            anchors.bottomMargin: Style.space(8)
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
              visible: Models.messageText(modelData).length > 0
              width: ListView.view ? ListView.view.width : 0
              height: visible ? bubble.height + Style.space(4) : 0

              Rectangle {
                id: bubble
                readonly property bool fromMe: modelData.is_from_me === true
                anchors.left: fromMe ? undefined : parent.left
                anchors.right: fromMe ? parent.right : undefined
                width: Math.round(parent.width * 0.78)
                height: bubbleText.implicitHeight + Style.space(16)
                radius: 8
                color: fromMe ? Qt.rgba(0.2, 0.5, 1, 0.35) : Qt.rgba(0, 0, 0, 0.08)
                border.width: fromMe ? 0 : 1
                border.color: fromMe ? "transparent" : root.dim

                Text {
                  id: bubbleText
                  anchors.left: parent.left
                  anchors.right: parent.right
                  anchors.top: parent.top
                  anchors.margins: Style.space(8)
                  wrapMode: Text.Wrap
                  text: Models.messageText(modelData)
                  color: root.barForeground
                  font.pixelSize: Style.font.body
                }
              }
            }
          }

          Text {
            anchors.centerIn: threadView
            width: threadView.width * 0.8
            visible: !imsg || !imsg.messages || imsg.messages.length === 0
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            color: root.barForeground
            opacity: 0.45
            font.pixelSize: Style.font.body
            text: selectedChatId > 0 ? "No messages in this chat yet." : "Select a conversation."
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
              color: root.wash
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

  Connections {
    target: imsg
    function onChatsChanged() {
      if (root.opened) root.maybeSelectFirst()
    }
  }
}
