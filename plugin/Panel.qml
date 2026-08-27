import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
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
  property var selectedChatId: ""
  property string draftText: ""
  property int phraseIndex: 0
  property bool settingsOpen: false
  property bool pinThreadToEnd: true
  property bool stickingThread: false
  property int stickGen: 0

  readonly property var barIdentity: hostWidget || root
  readonly property color fg: bar ? bar.foreground : Color.foreground
  readonly property string family: bar ? bar.fontFamily : Style.font.family
  readonly property color dim: Qt.darker(fg, 1.4)
  readonly property color urgent: Color.urgent
  readonly property color hoverFill: Style.hoverFillFor(fg, Color.accent)
  readonly property color selectedFill: Style.selectedFillFor(fg, Color.accent)
  readonly property color normalFill: Style.normalFillFor(fg, Color.accent)

  readonly property string currentTitle: {
    if (!imsg || !imsg.chats || !Models.hasId(selectedChatId)) return ""
    for (var i = 0; i < imsg.chats.length; i++) {
      if (Models.sameId(imsg.chats[i].id, selectedChatId)) return Models.chatTitle(imsg.chats[i])
    }
    return ""
  }
  readonly property var setupGuide: imsg && imsg.setupGuide ? imsg.setupGuide : Store.setupGuide({
    connected: false,
    cacheReady: false,
    statusKnown: false,
    bridgeConnected: false,
    contacts: "unknown",
    namesVisible: false,
    passwordSet: false
  })
  readonly property bool settingsVisible: root.settingsOpen || root.setupGuide.phase === "needs-settings"
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
    if (imsg.linkState === "mac-down") return "Showing saved messages. The Mac link is down."
    return ""
  }

  function maybeSelectFirst() {
    if (Models.hasId(selectedChatId) || !imsg || !imsg.chats || imsg.chats.length === 0) return
    openChat(imsg.chats[0].id)
  }

  function open() {
    root.controller.show()
    if (imsg) {
      imsg.refreshChats()
      imsg.refreshStatus()
      if (Models.hasId(selectedChatId)) openChat(selectedChatId)
      else maybeSelectFirst()
    }
  }

  function close() {
    if (imsg) imsg.openChatId = ""
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
    selectedChatId = String(chatId || "")
    draftText = ""
    root.pinThreadToEnd = true
    if (imsg) {
      imsg.openChatId = selectedChatId
      imsg.markRead(selectedChatId)
      imsg.loadMessages(selectedChatId, null)
    }
    root.stickThread()
  }

  function sendDraft() {
    if (!imsg || !Models.hasId(selectedChatId) || draftText.trim().length === 0 || imsg.sending) return
    var text = draftText
    root.draftText = ""
    if (draftField) draftField.text = ""
    imsg.sendMessage(selectedChatId, text)
    root.stickThread()
    root.focusComposer()
  }

  function pickAttachment() {
    if (!Models.hasId(selectedChatId) || (imsg && imsg.sending)) return
    photoDialog.open()
  }

  function focusComposer() {
    if (root.settingsVisible) return
    if (Models.hasId(selectedChatId) && draftField.enabled) draftField.forceActiveFocus()
    else keyCatcher.forceActiveFocus()
  }

  function blurComposer() {
    keyCatcher.forceActiveFocus()
  }

  function threadAtEnd() {
    if (!threadView || threadView.height <= 0) return true
    return (threadView.contentHeight - threadView.height - threadView.contentY) <= 24
  }

  function stickThread() {
    if (!root.pinThreadToEnd || !threadView) return
    var gen = ++root.stickGen
    root.stickingThread = true
    if (threadView.count > 0) threadView.positionViewAtIndex(threadView.count - 1, ListView.End)
    Qt.callLater(function () {
      if (gen !== root.stickGen) return
      if (root.pinThreadToEnd && threadView.count > 0)
        threadView.positionViewAtIndex(threadView.count - 1, ListView.End)
      root.stickingThread = false
    })
  }

  function composerKey(event) {
    if (event.key === Qt.Key_Escape) {
      root.blurComposer()
      event.accepted = true
      return
    }
    if (event.key === Qt.Key_Tab || event.key === Qt.Key_Backtab) {
      root.switchPanel((event.modifiers & Qt.ShiftModifier) || event.key === Qt.Key_Backtab ? -1 : 1)
      event.accepted = true
    }
  }

  function moveChat(delta) {
    if (!imsg || !imsg.chats || imsg.chats.length === 0 || delta === 0) return
    var i = 0
    for (; i < imsg.chats.length; i++) {
      if (Models.sameId(imsg.chats[i].id, selectedChatId)) break
    }
    if (i >= imsg.chats.length) i = 0
    var n = Math.max(0, Math.min(imsg.chats.length - 1, i + delta))
    openChat(imsg.chats[n].id)
    chatList.positionViewAtIndex(n, ListView.Contain)
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
    contentWidth: panel.fittedContentWidth(Style.space(720))
    contentHeight: panel.cappedContentHeight(Style.space(540))
    focusTarget: keyCatcher

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      clip: true
      blocked: settingsForm.editing || draftField.activeFocus
      onMoveRequested: function(dx, dy) {
        if (dx > 0) root.focusComposer()
        if (dy !== 0) root.moveChat(dy)
      }
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }
      onReturnRequested: root.focusComposer()
      onTextKey: function(t) {
        if (t === "a") root.pickAttachment()
      }

      Column {
        id: column
        anchors.fill: parent
        spacing: Style.space(12)

        Item {
          id: header
          width: parent.width
          implicitHeight: hero.implicitHeight
          height: hero.height
          function toggleSettings() {
            root.settingsOpen = !root.settingsOpen
          }

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
            trailingControl: Component {
              Button {
                iconText: "󰒓"
                tooltipText: "Settings"
                foreground: hero.foreground
                fontFamily: hero.fontFamily
                onClicked: header.toggleSettings()
              }
            }
          }
        }

        SettingsForm {
          id: settingsForm
          width: parent.width
          visible: root.settingsVisible
          serverUrl: imsg && imsg.settings ? imsg.settings.server_url : ""
          passwordSet: imsg && imsg.settings ? !!imsg.settings.password_set : false
          session: imsg && imsg.settings ? String(imsg.settings.session) : "unconfigured"
          saving: !!(imsg && imsg.settingsSaving)
          foreground: root.fg
          fontFamily: root.family
          lastError: imsg ? imsg.lastError : ""
          onSaveRequested: function (url, password) {
            if (imsg) imsg.saveSettings(url, password)
          }
          onReconnectRequested: if (imsg) imsg.reconnect()
        }

        Column {
          visible: !root.setupReady && !root.settingsVisible
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
          visible: root.setupReady && !root.settingsOpen
          width: parent.width
          height: parent.height - header.height - parent.spacing
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
              highlightFollowsCurrentItem: false
              currentIndex: -1
              focus: false
              activeFocusOnTab: false
              ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

              delegate: CursorSurface {
                required property var modelData
                width: chatList.width
                implicitHeight: chatInfo.implicitHeight + Style.spacing.rowPaddingX
                hasCursor: false
                current: Models.sameId(root.selectedChatId, modelData.id)
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
              model: imsg ? imsg.displayMessages : []
              clip: true
              spacing: Style.space(6)
              boundsBehavior: Flickable.StopAtBounds
              highlightFollowsCurrentItem: false
              currentIndex: -1
              focus: false
              activeFocusOnTab: false
              ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

              onCountChanged: if (root.pinThreadToEnd) root.stickThread()
              onContentHeightChanged: if (root.pinThreadToEnd) root.stickThread()
              onContentYChanged: {
                if (root.stickingThread) return
                if (moving || flicking || dragging) root.pinThreadToEnd = root.threadAtEnd()
              }
              onMovementEnded: if (!root.stickingThread) root.pinThreadToEnd = root.threadAtEnd()

              header: Item {
                width: threadView.width
                height: Models.hasId(selectedChatId) && imsg && imsg.messages && imsg.messages.length > 0 ? Style.space(36) : 0
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
                    if (oldest && oldest.created_at) {
                      root.pinThreadToEnd = false
                      imsg.loadMessages(selectedChatId, oldest.created_at)
                    }
                  }
                }
              }

              delegate: Item {
                visible: Models.messageText(modelData).length > 0 || Models.hasLocalPhoto(modelData)
                width: ListView.view ? ListView.view.width : 0
                height: visible ? bubble.height : 0

                BorderSurface {
                  id: bubble
                  readonly property bool fromMe: modelData.is_from_me === true
                  anchors.left: fromMe ? undefined : parent.left
                  anchors.right: fromMe ? parent.right : undefined
                  width: Math.round(parent.width * 0.78)
                  implicitHeight: bubbleCol.implicitHeight + Style.space(16)
                  radius: Style.cornerRadius
                  color: fromMe ? root.selectedFill : root.normalFill
                  borderSpec: fromMe
                    ? Border.none()
                    : Border.controlSpec("normal", root.fg, Color.accent)

                  Column {
                    id: bubbleCol
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: parent.top
                    anchors.margins: Style.space(8)
                    spacing: Style.space(4)

                    Image {
                      visible: Models.hasLocalPhoto(modelData)
                      width: parent.width
                      height: visible ? Math.min(Style.space(180), implicitHeight > 0 ? implicitHeight : Style.space(120)) : 0
                      fillMode: Image.PreserveAspectFit
                      asynchronous: true
                      source: visible ? ("file://" + String(modelData.local_path)) : ""
                    }

                    Text {
                      id: bubbleText
                      width: parent.width
                      visible: Models.messageText(modelData).length > 0
                      wrapMode: Text.Wrap
                      text: Models.messageText(modelData)
                      color: root.fg
                      font.family: root.family
                      font.pixelSize: Style.font.body
                    }

                    Text {
                      width: parent.width
                      visible: bubble.fromMe && (modelData.send_state === "sending" || modelData.send_state === "failed")
                      text: modelData.send_state === "failed" ? "Not delivered" : "Sending"
                      color: modelData.send_state === "failed" ? root.urgent : root.dim
                      font.family: root.family
                      font.pixelSize: Style.font.caption
                    }
                  }
                }
              }
            }

            Text {
              anchors.centerIn: threadView
              width: threadView.width * 0.8
              visible: (!imsg || !imsg.displayMessages || imsg.displayMessages.length === 0) && root.setupReady
              horizontalAlignment: Text.AlignHCenter
              wrapMode: Text.WordWrap
              color: root.dim
              font.family: root.family
              font.pixelSize: Style.font.body
              text: Models.hasId(selectedChatId) ? "No messages in this chat yet." : "Select a conversation."
            }

            Row {
              id: composerRow
              anchors.left: parent.left
              anchors.right: parent.right
              anchors.bottom: parent.bottom
              spacing: Style.space(8)

              Button {
                id: photoBtn
                text: "Photo"
                foreground: root.fg
                fontFamily: root.family
                enabled: Models.hasId(selectedChatId) && imsg && !imsg.sending
                onClicked: root.pickAttachment()
              }

              TextField {
                id: draftField
                width: parent.width - sendBtn.width - photoBtn.width - parent.spacing * 2
                foreground: root.fg
                placeholderText: Models.hasId(selectedChatId) ? "Message" : "Select a conversation"
                enabled: Models.hasId(selectedChatId)
                text: root.draftText
                onTextChanged: root.draftText = text
                onAccepted: root.sendDraft()
                Keys.priority: Keys.BeforeItem
                Keys.onPressed: root.composerKey(event)
              }

              Button {
                id: sendBtn
                text: imsg && imsg.sending ? "…" : "Send"
                foreground: root.fg
                fontFamily: root.family
                enabled: Models.hasId(selectedChatId) && imsg && !imsg.sending && root.draftText.trim().length > 0
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
      if (!imsg || !Models.hasId(selectedChatId) || !imsg.chats) return
      for (var i = 0; i < imsg.chats.length; i++) {
        if (Models.sameId(imsg.chats[i].id, selectedChatId) && (imsg.chats[i].unread_count || 0) > 0) {
          imsg.markRead(selectedChatId)
          break
        }
      }
    }
    function onMessagesChanged() {
      if (root.pinThreadToEnd) root.stickThread()
    }
    function onDisplayMessagesChanged() {
      if (root.pinThreadToEnd) root.stickThread()
    }
    function onFailedDraftChanged() {
      if (!imsg || !imsg.failedDraft || imsg.failedDraft.length === 0) return
      if (root.draftText.trim().length > 0) return
      root.draftText = imsg.failedDraft
      if (draftField) draftField.text = imsg.failedDraft
    }
  }

  FileDialog {
    id: photoDialog
    title: "Send photo"
    fileMode: FileDialog.OpenFile
    nameFilters: ["Images (*.jpg *.jpeg *.png *.gif *.webp *.heic *.heif *.bmp)"]
    onAccepted: {
      var u = String(selectedFile)
      if (u.indexOf("file://") === 0) u = decodeURIComponent(u.substring(7))
      if (imsg) imsg.sendAttachment(selectedChatId, u)
      root.stickThread()
    }
  }
}
