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
    },
    settings: settingsFrom(result)
  }
}

function settingsFrom(result) {
  return {
    server_url: (result && result.server_url) ? String(result.server_url) : "",
    password_set: !!(result && result.password_set),
    session: (result && result.session) ? String(result.session) : "unconfigured",
    webhook_enabled: !!(result && result.webhook_enabled),
    webhook_port: result && result.webhook_port ? Number(result.webhook_port) : 18792,
    webhook_serve_url: (result && result.webhook_serve_url) ? String(result.webhook_serve_url) : "",
    webhook_listening: !!(result && result.webhook_listening),
    webhook_registered: !!(result && result.webhook_registered),
    webhook_token_set: !!(result && result.webhook_token_set)
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
    var payload = event.payload || {}
    var patch = { link: payload }
    if (payload.server_url !== undefined || payload.password_set !== undefined || payload.session !== undefined) {
      patch.settings = settingsFrom(payload)
    }
    return patch
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
  if (msg && state.openChatId && String(msg.chat_id) === String(state.openChatId)) {
    patch.messages = appendMessage(state.messages || [], msg)
  }
  if (payload.is_new && msg && msg.is_from_me !== true && String(msg.chat_id) !== String(state.openChatId)) {
    patch.notify = {
      sender: notifySender(msg, payload.chat || findChat(state.chats, msg.chat_id)),
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
    if (String(chats[i].id) === String(chat.id)) {
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
    if (String(messages[i].id) === String(msg.id)) {
      var copy = messages.slice()
      copy[i] = msg
      return copy
    }
  }
  return messages.concat([msg])
}

function isPersonName(value) {
  return /[A-Za-z]/.test(String(value || ""))
}

function findChat(chats, chatId) {
  chats = chats || []
  for (var i = 0; i < chats.length; i++) {
    if (String(chats[i].id) === String(chatId || "")) return chats[i]
  }
  return null
}

function notifySender(msg, chat) {
  if (isPersonName(msg && msg.sender_name)) return msg.sender_name
  if (isPersonName(chat && chat.contact_name)) return chat.contact_name
  if (isPersonName(chat && chat.display_name)) return chat.display_name
  if (isPersonName(chat && chat.name)) return chat.name
  return String((msg && msg.sender) || "iMessage")
}

function totalUnread(chats) {
  var n = 0
  for (var i = 0; i < chats.length; i++) n += chats[i].unread_count || 0
  return n
}

function linkState(s) {
  s = s || {}
  if (!s.connected && !s.cacheReady) return "waiting"
  if (!s.connected) return "sync-down"
  if (!s.statusKnown) return "checking"
  if (s.bridgeConnected) return "live"
  return "mac-down"
}

function setupGuide(s) {
  s = s || {}
  if (s.cacheReady) {
    return {
      phase: "ready",
      title: "",
      body: "",
      hint: "",
      actionKind: (s.contacts === "unavailable" && !s.namesVisible) ? "contacts" : ""
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
  if (!s.passwordSet) {
    return {
      phase: "needs-settings",
      title: "Link this machine",
      body: "BlueBubbles URL and password. Saved in the system keyring.",
      hint: "",
      actionKind: "settings"
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
  if (s.bridgeConnected && !s.cacheReady) {
    return {
      phase: "loading",
      title: "Loading conversations",
      body: "The Mac link is up. Chats appear here in a moment.",
      hint: "",
      actionKind: ""
    }
  }
  return {
    phase: "needs-mac",
    title: "This machine is not linked",
    body: "BlueBubbles is running on the Mac. Point this machine at it in Settings.",
    hint: "",
    actionKind: "settings"
  }
}

function webhookGuide(s) {
  s = s || {}
  var enabled = !!s.enabled
  var listening = !!s.listening
  var registered = !!s.registered
  var live = s.session === "live"
  var serveOffered = !!s.serveOffered

  if (enabled && listening && registered) {
    return {
      phase: "ready",
      step: 3,
      steps: 3,
      title: "Listening. Registered with BlueBubbles. Poll is off.",
      body: "",
      actionKind: "",
      actionLabel: ""
    }
  }
  if (!enabled) {
    return {
      phase: "needs-enable",
      step: 1,
      steps: 3,
      title: "Turn on the webhook",
      body: "BlueBubbles pokes this machine. We then pull the real iMessage with your password. Poll turns off.",
      actionKind: "enable",
      actionLabel: "Turn on webhook"
    }
  }
  if (!listening) {
    return {
      phase: "waiting",
      step: 1,
      steps: 3,
      title: "Starting the listener",
      body: "Hang on a second.",
      actionKind: "",
      actionLabel: ""
    }
  }
  if (!serveOffered) {
    return {
      phase: "needs-serve",
      step: 2,
      steps: 3,
      title: "Publish on Tailscale",
      body: "Opens the Omarchy window and publishes localhost on your tailnet. Not Funnel. Restrict the Serve ACL to your Mac.",
      actionKind: "serve",
      actionLabel: "Publish with Tailscale"
    }
  }
  if (!live) {
    return {
      phase: "needs-live",
      step: 3,
      steps: 3,
      title: "Connect BlueBubbles first",
      body: "Register needs a live Mac link. Save the URL and password above, then come back.",
      actionKind: "reconnect",
      actionLabel: "Reconnect"
    }
  }
  return {
    phase: "needs-register",
    step: 3,
    steps: 3,
    title: "Register with BlueBubbles",
    body: "BlueBubbles will not poke this machine until you register.",
    actionKind: "register",
    actionLabel: "Register webhook"
  }
}
