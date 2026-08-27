import QtQuick
import QtQuick.Controls
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
  property int phraseIndex: 0

  readonly property var barIdentity: hostWidget || root
  readonly property color fg: bar ? bar.foreground : Color.foreground
  readonly property string family: bar ? bar.fontFamily : Style.font.family
  readonly property color dim: Qt.darker(fg, 1.4)
  readonly property color urgent: Color.urgent
  readonly property color hoverFill: Style.hoverFillFor(fg, Color.accent)
  readonly property color selectedFill: Style.selectedFillFor(fg, Color.accent)
  readonly property color normalFill: Style.normalFillFor(fg, Color.accent)

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
  readonly property var livePhrases: [
    "Delivering bubbles",
    "Keeping the thread",
    "Sorting pings",
    "Reading the tape"
  ]
  readonly property string heroMeta: {
    if (!imsg) return "Starting"
    if (imsg.linkState === "live") return livePhrases[phraseIndex % livePhrases.length]
    if (imsg.linkState === "mac-locked") return "Messages is locked"
    if (imsg.linkState === "mac-down") return "Mac link is down"
    if (imsg.linkState === "sync-down") return "Sync is down"
    if (imsg.linkState === "checking") return "Checking the Mac"
    return "Waiting"
  }
  readonly property string heroDetail: {
    if (!imsg || !imsg.unreadCount) return ""
    return String(imsg.unreadCount)
  }
  readonly property string statusLine: {
    if (!imsg) return ""
    if (imsg.sendError && imsg.sendError.length > 0) return imsg.sendError
    if (imsg.linkState === "mac-locked") return "Grant Full Disk Access to BlueBubbles on the Mac."
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

  function switchPanel(direction) {
    if (root.bar && typeof root.bar.switchPanelFrom === "function")
      return root.bar.switchPanelFrom(root.barIdentity, direction)
    return false
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

  function moveChat(delta) {
    if (!imsg || !imsg.chats || imsg.chats.length === 0 || delta === 0) return
    var i = 0
    for (; i < imsg.chats.length; i++) {
      if (imsg.chats[i].id === selectedChatId) break
    }
    if (i >= imsg.chats.length) i = 0
    var n = Math.max(0, Math.min(imsg.chats.length - 1, i + delta))
    openChat(imsg.chats[n].id)
  }

  function call(method, args) {
    if (method === "openChat" && args && args.chat_id) {
      open()
      openChat(args.chat_id)
      return "ok"
    }
    return "unknown"
  }

  Timer {
    id: phraseTimer
    interval: 2800
    running: root.opened && imsg && imsg.linkState === "live"
    repeat: true
    onTriggered: phraseSwap.restart()
  }

  SequentialAnimation {
    id: phraseSwap
    PropertyAnimation {
      target: hero
      property: "metaOpacity"
      to: 0.0
      duration: 180
      easing.type: Easing.OutQuad
    }
    ScriptAction {
      script: root.phraseIndex = (root.phraseIndex + 1) % root.livePhrases.length
    }
    PropertyAnimation {
      target: hero
      property: "metaOpacity"
      to: 1.0
      duration: 260
      easing.type: Easing.InQuad
    }
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
      onMoveRequested: function(dx, dy) {
        if (dy !== 0) root.moveChat(dy)
      }
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }

      Column {
        id: column
        anchors.fill: parent
        spacing: Style.space(12)

        PanelHero {
          id: hero
          width: parent.width
          title: "iMessage"
          meta: root.heroMeta
          detail: root.heroDetail
          foreground: root.fg
          fontFamily: root.family
          iconOpacity: imsg && imsg.linkState === "live" ? 1.0 : 0.55
          iconComponent: Component {
            Text {
              text: "󰍩"
              color: root.fg
              font.family: root.family
              font.pixelSize: Style.font.display
            }
          }
        }

        Column {
          visible: !root.setupReady
          width: parent.width
          spacing: Style.space(12)

          Text {
            width: parent.width
            topPadding: Style.space(24)
            text: root.setupGuide.title
            color: root.fg
            font.family: root.family
            font.pixelSize: Style.font.title
            font.bold: true
            wrapMode: Text.WordWrap
            horizontalAlignment: Text.AlignHCenter
          }
          Text {
            width: parent.width
            text: root.setupGuide.body
            color: root.dim
            font.family: root.family
            font.pixelSize: Style.font.body
            wrapMode: Text.WordWrap
            horizontalAlignment: Text.AlignHCenter
          }
          TextEdit {
            width: parent.width
            visible: root.setupGuide.hint && root.setupGuide.hint.length > 0
            text: root.setupGuide.hint || ""
            color: root.dim
            readOnly: true
            selectByMouse: true
            wrapMode: TextEdit.Wrap
            font.family: root.family
            font.pixelSize: Style.font.caption
            horizontalAlignment: TextEdit.AlignHCenter
          }
        }

        Row {
          id: panes
          visible: root.setupReady
          width: parent.width
          height: parent.height - hero.height - parent.spacing
          spacing: Style.space(12)

          Column {
            id: chatPane
            width: Math.max(Style.space(220), Math.floor(parent.width * 0.32))
            height: parent.height
            spacing: Style.space(10)

            PanelSectionHeader {
              id: chatsHeader
              width: parent.width
              text: "CHATS"
              foreground: root.fg
              fontFamily: root.family
            }

            ListView {
              id: chatList
              width: parent.width
              height: parent.height - chatsHeader.height - parent.spacing
              model: imsg ? imsg.chats : []
              clip: true
              spacing: Style.space(6)
              boundsBehavior: Flickable.StopAtBounds
              ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

              delegate: CursorSurface {
                required property var modelData
                width: chatList.width
                implicitHeight: chatInfo.implicitHeight + Style.spacing.rowPaddingX
                hasCursor: false
                current: root.selectedChatId === modelData.id
                foreground: root.fg
                fill: root.hoverFill
                currentFill: root.selectedFill

                MouseArea {
                  anchors.fill: parent
                  hoverEnabled: true
                  cursorShape: Qt.PointingHandCursor
                  onClicked: root.openChat(modelData.id)
                }

                Item {
                  id: chatInfo
                  anchors.left: parent.left
                  anchors.right: parent.right
                  anchors.verticalCenter: parent.verticalCenter
                  anchors.leftMargin: Style.space(10)
                  anchors.rightMargin: Style.space(10)
                  implicitHeight: Math.max(chatName.implicitHeight + chatMeta.implicitHeight + Style.space(2), Style.space(28))

                  Text {
                    id: chatName
                    anchors.left: parent.left
                    anchors.right: unreadPill.left
                    anchors.rightMargin: unreadPill.visible ? Style.space(8) : 0
                    anchors.top: parent.top
                    text: Models.chatTitle(modelData)
                    color: root.fg
                    font.family: root.family
                    font.pixelSize: Style.font.body
                    font.bold: (modelData.unread_count || 0) > 0
                    elide: Text.ElideRight
                  }

                  Text {
                    id: chatMeta
                    anchors.left: parent.left
                    anchors.right: unreadPill.left
                    anchors.rightMargin: unreadPill.visible ? Style.space(8) : 0
                    anchors.top: chatName.bottom
                    anchors.topMargin: Style.space(1)
                    text: Models.formatTime(modelData.last_message_at)
                    color: root.dim
                    font.family: root.family
                    font.pixelSize: Style.font.caption
                    elide: Text.ElideRight
                  }

                  BorderSurface {
                    id: unreadPill
                    visible: (modelData.unread_count || 0) > 0
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    implicitWidth: unreadText.implicitWidth + Style.space(10)
                    implicitHeight: unreadText.implicitHeight + Style.space(4)
                    color: "transparent"
                    borderSpec: Border.controlSpec("normal", root.fg, Color.accent)
                    radius: Style.cornerRadius

                    Text {
                      id: unreadText
                      anchors.centerIn: parent
                      text: String(modelData.unread_count || 0)
                      color: root.dim
                      font.family: root.family
                      font.pixelSize: Style.font.caption
                      font.bold: true
                    }
                  }
                }
              }
            }
          }

          Rectangle {
            width: 1
            height: parent.height
            color: Qt.rgba(root.fg.r, root.fg.g, root.fg.b, 0.12)
          }

          Item {
            id: threadPane
            width: parent.width - chatPane.width - parent.spacing * 2 - 1
            height: parent.height

            Column {
              id: threadHeader
              anchors.top: parent.top
              width: parent.width
              spacing: Style.space(8)

              Text {
                width: parent.width
                visible: root.statusLine.length > 0
                text: root.statusLine
                color: imsg && imsg.sendError && imsg.sendError.length > 0 ? root.urgent : root.dim
                font.family: root.family
                font.pixelSize: Style.font.bodySmall
                wrapMode: Text.WordWrap
              }

              Button {
                width: parent.width
                visible: root.setupGuide.actionKind === "contacts"
                text: "Show contact names"
                foreground: root.fg
                fontFamily: root.family
                bordered: true
                onClicked: if (imsg) imsg.requestContactsAccess()
              }

              Text {
                width: parent.width
                visible: imsg && imsg.contactsState === "prompting"
                text: "Click Allow on your Mac to show contact names."
                color: root.dim
                font.family: root.family
                font.pixelSize: Style.font.caption
                wrapMode: Text.WordWrap
              }

              PanelSectionHeader {
                width: parent.width
                visible: root.currentTitle.length > 0
                text: root.currentTitle
                foreground: root.fg
                fontFamily: root.family
              }
            }

            ListView {
              id: threadView
              anchors.top: threadHeader.bottom
              anchors.topMargin: Style.space(8)
              anchors.left: parent.left
              anchors.right: parent.right
              anchors.bottom: composerRow.top
              anchors.bottomMargin: Style.space(8)
              model: imsg ? imsg.messages : []
              clip: true
              spacing: Style.space(6)
              boundsBehavior: Flickable.StopAtBounds
              ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

              onCountChanged: if (count > 0) positionViewAtEnd()

              header: Item {
                width: threadView.width
                height: selectedChatId > 0 && imsg && imsg.messages && imsg.messages.length > 0 ? Style.space(36) : 0
                Button {
                  anchors.horizontalCenter: parent.horizontalCenter
                  visible: parent.height > 0
                  text: "Load older"
                  foreground: root.fg
                  fontFamily: root.family
                  fontSize: Style.font.caption
                  bordered: true
                  onClicked: {
                    if (!imsg || imsg.messages.length === 0) return
                    var oldest = imsg.messages[0]
                    if (oldest && oldest.created_at) imsg.loadMessages(selectedChatId, oldest.created_at)
                  }
                }
              }

              delegate: Item {
                visible: Models.messageText(modelData).length > 0
                width: ListView.view ? ListView.view.width : 0
                height: visible ? bubble.height : 0

                BorderSurface {
                  id: bubble
                  readonly property bool fromMe: modelData.is_from_me === true
                  anchors.left: fromMe ? undefined : parent.left
                  anchors.right: fromMe ? parent.right : undefined
                  width: Math.round(parent.width * 0.78)
                  implicitHeight: bubbleText.implicitHeight + Style.space(16)
                  radius: Style.cornerRadius
                  color: fromMe ? root.selectedFill : root.normalFill
                  borderSpec: fromMe
                    ? Border.none()
                    : Border.controlSpec("normal", root.fg, Color.accent)

                  Text {
                    id: bubbleText
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: parent.top
                    anchors.margins: Style.space(8)
                    wrapMode: Text.Wrap
                    text: Models.messageText(modelData)
                    color: root.fg
                    font.family: root.family
                    font.pixelSize: Style.font.body
                  }
                }
              }
            }

            Text {
              anchors.centerIn: threadView
              width: threadView.width * 0.8
              visible: (!imsg || !imsg.messages || imsg.messages.length === 0) && root.setupReady
              horizontalAlignment: Text.AlignHCenter
              wrapMode: Text.WordWrap
              color: root.dim
              font.family: root.family
              font.pixelSize: Style.font.body
              text: selectedChatId > 0 ? "No messages in this chat yet." : "Select a conversation."
            }

            Row {
              id: composerRow
              anchors.left: parent.left
              anchors.right: parent.right
              anchors.bottom: parent.bottom
              spacing: Style.space(8)

              TextField {
                id: draftField
                width: parent.width - sendBtn.width - parent.spacing
                foreground: root.fg
                placeholderText: selectedChatId > 0 ? "Message" : "Select a conversation"
                enabled: selectedChatId > 0 && imsg && !imsg.sending
                text: root.draftText
                onTextChanged: root.draftText = text
                onAccepted: root.sendDraft()
              }

              Button {
                id: sendBtn
                text: imsg && imsg.sending ? "…" : "Send"
                foreground: root.fg
                fontFamily: root.family
                enabled: selectedChatId > 0 && imsg && !imsg.sending && root.draftText.trim().length > 0
                onClicked: root.sendDraft()
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
