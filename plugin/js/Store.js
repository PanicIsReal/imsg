.pragma library

function applySnapshot(result) {
  var chats = (result && result.chats) ? result.chats : []
  return {
    chats: chats,
    unreadCount: totalUnread(chats),
    link: {
      bridge_connected: !!(result && result.bridge_connected),
      database_ready: !!(result && result.database_ready),
      last_error: (result && result.last_error) ? result.last_error : "",
      contacts: (result && result.contacts) ? result.contacts : "unknown"
    }
  }
}

function applyEvent(state, event) {
  if (!event || event.type !== "event") return {}
  if (event.topic === "sync.message") return applyMessage(state, event.payload || {})
  if (event.topic === "sync.chats") {
    var chats = event.payload && event.payload.chats ? event.payload.chats : []
    return { chats: chats, unreadCount: totalUnread(chats) }
  }
  if (event.topic === "sync.link") {
    return { link: event.payload || {} }
  }
  return {}
}

function applyMessage(state, payload) {
  var patch = {}
  if (payload.chat) {
    patch.chats = upsertChat(state.chats || [], payload.chat)
    patch.unreadCount = totalUnread(patch.chats)
  }
  var msg = payload.message
  if (msg && state.openChatId && Number(msg.chat_id) === Number(state.openChatId)) {
    patch.messages = appendMessage(state.messages || [], msg)
  }
  if (payload.is_new && msg && msg.is_from_me !== true && Number(msg.chat_id) !== Number(state.openChatId)) {
    patch.notify = {
      sender: msg.sender_name || msg.sender || "iMessage",
      preview: msg.text || "",
      chatId: msg.chat_id
    }
  }
  return patch
}

function upsertChat(chats, chat) {
  var out = []
  var found = false
  for (var i = 0; i < chats.length; i++) {
    if (chats[i].id === chat.id) {
      out.push(chat)
      found = true
    } else {
      out.push(chats[i])
    }
  }
  if (!found) out.push(chat)
  out.sort(function (a, b) {
    var at = a.last_message_at || ""
    var bt = b.last_message_at || ""
    if (at === bt) return 0
    return at < bt ? 1 : -1
  })
  return out
}

function appendMessage(messages, msg) {
  for (var i = 0; i < messages.length; i++) {
    if (messages[i].id === msg.id) {
      var copy = messages.slice()
      copy[i] = msg
      return copy
    }
  }
  return messages.concat([msg])
}

function totalUnread(chats) {
  var n = 0
  for (var i = 0; i < chats.length; i++) n += chats[i].unread_count || 0
  return n
}

function setupGuide(s) {
  s = s || {}
  if (s.cacheReady) {
    return {
      phase: "ready",
      title: "",
      body: "",
      hint: "",
      actionKind: s.contacts === "unavailable" ? "contacts" : ""
    }
  }
  if (!s.connected) {
    return {
      phase: "needs-sync",
      title: "iMessage is not running here yet",
      body: "Start the local sync service. This panel fills in from your Mac after that.",
      hint: "imsg sync run",
      actionKind: ""
    }
  }
  if (!s.statusKnown) {
    return {
      phase: "checking",
      title: "Checking the Mac link…",
      body: "Hang on a second.",
      hint: "",
      actionKind: ""
    }
  }
  if (s.bridgeConnected && !s.databaseReady) {
    return {
      phase: "needs-fda",
      title: "Messages is locked on your Mac",
      body: "Grant Full Disk Access to Ghostty, the window titled imsg-bridge-serve. The list appears after that.",
      hint: "",
      actionKind: ""
    }
  }
  return {
    phase: "needs-mac",
    title: "This machine is not linked",
    body: "Pair Omarchy with the Mac. On the Mac run a pairing rotate, then run the command below here.",
    hint: "imsg setup pair <code> --host <mac-tailscale-ip>",
    actionKind: ""
  }
}
