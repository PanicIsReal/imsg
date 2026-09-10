.pragma library

function shouldResetGeneration(previous, next, explicitReset) {
  if (previous && next) return previous !== next
  return explicitReset === true
}

function syncListModel(model, messages) {
  for (var i = 0; i < messages.length; i++) {
    var id = String(messages[i].id)
    var found = i
    while (found < model.count && String(model.get(found).entry.id) !== id) found++
    if (found === model.count) {
      model.insert(i, { entry: messages[i] })
    } else {
      if (found !== i) model.move(found, i, 1)
      if (JSON.stringify(model.get(i).entry) !== JSON.stringify(messages[i])) {
        model.set(i, { entry: messages[i] })
      }
    }
  }
  if (model.count > messages.length) model.remove(messages.length, model.count - messages.length)
}

function applySnapshot(result) {
  var chats = (result && result.chats) ? result.chats : []
  return {
    chats: chats,
    databaseGeneration: result && result.db_generation ? String(result.db_generation) : "",
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
  if (event.topic === "sync.chats" || event.topic === "sync.resync") {
    var payload = event.payload || {}
    var patch = {}
    if (payload.db_generation) patch.databaseGeneration = String(payload.db_generation)
    if (payload.chats) {
      patch.chats = payload.chats
      patch.unreadCount = totalUnread(payload.chats)
    }
    if (event.topic === "sync.resync" || payload.reason === "events_lagged") patch.resyncOpenChat = true
    if (event.topic === "sync.resync" && payload.reason === "db_generation") patch.resetGeneration = true
    return patch
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
  if (msg && msg.chat_id) {
    var matchedClientId = msg.client_id || clientIdForServerId(state.outbox || [], msg.id)
    if (matchedClientId && msg.is_from_me !== true) {
      msg = cloneObject(msg)
      msg.is_from_me = true
      msg.client_id = matchedClientId
    }
    patch.messagesByChat = mergeLive(state.messagesByChat || {}, msg.chat_id, msg)
    if (matchedClientId) patch.outbox = markOutbox(state.outbox || [], matchedClientId, "sent", "", msg.id)
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

function enqueue(outbox, chatId, text, clientId, createdAt) {
  return (outbox || []).concat([{
    client_id: clientId,
    chat_id: chatId,
    text: text,
    status: "queued",
    error: "",
    created_at: createdAt
  }])
}

function nextQueued(outbox) {
  for (var i = 0; i < (outbox || []).length; i++) if (outbox[i].status === "queued") return outbox[i]
  return null
}

function outboxStatus(outbox, clientId) {
  for (var i = 0; i < (outbox || []).length; i++) {
    if (outbox[i].client_id === clientId) return outbox[i].status
  }
  return ""
}

function markOutbox(outbox, clientId, status, error, serverId) {
  var copy = (outbox || []).slice()
  for (var i = 0; i < copy.length; i++) {
    if (copy[i].client_id !== clientId) continue
    var item = cloneObject(copy[i])
    item.status = status
    item.error = error || ""
    if (serverId !== undefined && serverId !== null) item.server_id = serverId
    copy[i] = item
    break
  }
  return copy
}

function acknowledge(outbox, clientId, result) {
  var message = result && result.message ? result.message : result
  var serverId = message && message.id !== undefined ? message.id : null
  for (var i = 0; i < (outbox || []).length; i++) {
    if (outbox[i].client_id === clientId && outbox[i].status === "sent") return outbox
  }
  return serverId !== null
    ? markOutbox(outbox, clientId, "sent", "", serverId)
    : markOutbox(outbox, clientId, "unconfirmed", "Send not confirmed", null)
}

function retryAs(outbox, clientId, nextClientId, createdAt) {
  var copy = (outbox || []).slice()
  for (var i = 0; i < copy.length; i++) {
    if (copy[i].client_id !== clientId) continue
    if (copy[i].retry_allowed === false || (copy[i].status !== "failed" && copy[i].status !== "unconfirmed")) return outbox
    var item = cloneObject(copy[i])
    delete item.server_id
    item.client_id = nextClientId
    item.created_at = createdAt
    item.status = "queued"
    item.error = ""
    copy[i] = item
    break
  }
  return copy
}

function failOutbox(outbox, clientId, status, error) {
  for (var i = 0; i < (outbox || []).length; i++) {
    if (outbox[i].client_id === clientId && outbox[i].status === "sent") return outbox
  }
  return markOutbox(outbox, clientId, status, error, null)
}

function classifySendFailure(error) {
  var code = String(error && error.code || "").toLowerCase()
  var message = String(error && error.message || "").toLowerCase()
  if (
    code === "timeout" ||
    code === "transport" ||
    code === "sync_down" ||
    code === "link_down" ||
    code === "upstream" ||
    code === "upstream_error"
  ) return "unconfirmed"
  if (message.indexOf("timeout") !== -1 || message.indexOf("timed out") !== -1 || message.indexOf("restart") !== -1) return "unconfirmed"
  if (message.indexOf("connection") !== -1 || message.indexOf("transport") !== -1) return "unconfirmed"
  if (message.indexOf("generation") !== -1 || message.indexOf("unconfirmed") !== -1) return "unconfirmed"
  return "failed"
}

function resetOutboxGeneration(outbox) {
  var copy = []
  for (var i = 0; i < (outbox || []).length; i++) {
    if (outbox[i].status === "sent") continue
    var item = cloneObject(outbox[i])
    delete item.server_id
    item.status = "unconfirmed"
    item.error = "Conversation database changed. Copy the text into a newly selected conversation."
    item.retry_allowed = false
    copy.push(item)
  }
  return copy
}

function clientIdForServerId(outbox, serverId) {
  if (serverId === undefined || serverId === null) return ""
  for (var i = 0; i < (outbox || []).length; i++) {
    if (outbox[i].server_id !== undefined && String(outbox[i].server_id) === String(serverId)) return outbox[i].client_id
  }
  return ""
}

function messagesForChat(messagesByChat, outbox, chatId) {
  var messages = ((messagesByChat || {})[chatKey(chatId)] || []).slice()
  var delivered = {}
  var serverIds = {}
  for (var i = 0; i < messages.length; i++) {
    if (messages[i].client_id) delivered[messages[i].client_id] = true
    if (messages[i].id !== undefined && messages[i].id !== null) serverIds[String(messages[i].id)] = true
  }
  for (var j = 0; j < (outbox || []).length; j++) {
    var entry = outbox[j]
    if (Number(entry.chat_id) !== Number(chatId) || delivered[entry.client_id]) continue
    if (entry.server_id !== undefined && serverIds[String(entry.server_id)]) continue
    messages.push({
      id: "local:" + entry.client_id,
      client_id: entry.client_id,
      chat_id: entry.chat_id,
      text: entry.text,
      is_from_me: true,
      created_at: entry.created_at,
      delivery_status: entry.status,
      delivery_error: entry.error || "",
      retry_allowed: entry.retry_allowed !== false,
      local_pending: true
    })
  }
  return sortMessages(messages)
}

function mergeHistory(messagesByChat, chatId, history, replaceExisting) {
  var byChat = cloneObject(messagesByChat || {})
  var key = chatKey(chatId)
  byChat[key] = mergeMessages(byChat[key] || [], history || [], replaceExisting === true)
  return byChat
}

function mergeLive(messagesByChat, chatId, message) {
  var byChat = cloneObject(messagesByChat || {})
  var key = chatKey(chatId)
  byChat[key] = mergeMessages(byChat[key] || [], [message], true)
  return byChat
}

function mergeMessages(current, incoming, replaceExisting) {
  var out = current.slice()
  var indexes = {}
  for (var i = 0; i < out.length; i++) {
    if (out[i].id !== undefined && out[i].id !== null) indexes[String(out[i].id)] = i
  }
  for (var j = 0; j < incoming.length; j++) {
    var msg = incoming[j]
    var id = msg && msg.id !== undefined && msg.id !== null ? String(msg.id) : ""
    if (id && indexes[id] !== undefined) {
      if (replaceExisting) out[indexes[id]] = msg
    } else {
      if (id) indexes[id] = out.length
      out.push(msg)
    }
  }
  return sortMessages(out)
}

function sortMessages(messages) {
  return messages.slice().sort(function(a, b) {
    var at = a.created_at || ""
    var bt = b.created_at || ""
    if (at === bt) return String(a.id || "") < String(b.id || "") ? -1 : 1
    return at < bt ? -1 : 1
  })
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

function totalUnread(chats) {
  var n = 0
  for (var i = 0; i < chats.length; i++) n += chats[i].unread_count || 0
  return n
}
function chatKey(chatId) { return String(Number(chatId)) }
function cloneObject(value) {
  var copy = {}
  for (var key in value) copy[key] = value[key]
  return copy
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
      body: "Grant Full Disk Access to imsg on the Mac. The list appears after that.",
      hint: "",
      actionKind: ""
    }
  }
  return {
    phase: "needs-mac",
    title: "This machine is not linked",
    body: "The Mac needs Homebrew imsg and a running bridge. After that, pair from here with imsg setup pair <code> --host <mac-tailscale-ip>.",
    hint: "brew install steipete/tap/imsg",
    actionKind: ""
  }
}
