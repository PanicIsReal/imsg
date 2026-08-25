.pragma library

function chatTitle(chat) {
  if (!chat) return "Chat"
  if (chat.contact_name && chat.contact_name.length > 0) return chat.contact_name
  if (chat.display_name && chat.display_name.length > 0) return chat.display_name
  if (chat.name && chat.name.length > 0) return chat.name
  if (chat.is_group && chat.participants && chat.participants.length > 0) {
    var names = []
    for (var i = 0; i < Math.min(chat.participants.length, 3); i++) {
      names.push(formatHandle(chat.participants[i]))
    }
    var suffix = chat.participants.length > 3 ? "…" : ""
    return names.join(", ") + suffix
  }
  if (chat.identifier) return formatHandle(chat.identifier)
  return "Chat " + chat.id
}

function formatHandle(value) {
  if (!value) return ""
  var s = String(value)
  if (s.indexOf("+1") === 0 && s.length === 12) {
    return "+1 (" + s.substring(2, 5) + ") " + s.substring(5, 8) + "-" + s.substring(8)
  }
  return s
}

function formatTime(iso) {
  if (!iso) return ""
  var d = new Date(iso)
  if (isNaN(d.getTime())) return iso
  var now = new Date()
  var sameDay = d.getFullYear() === now.getFullYear()
    && d.getMonth() === now.getMonth()
    && d.getDate() === now.getDate()
  if (sameDay) {
    return Qt.formatTime(d, "h:mm AP")
  }
  var yesterday = new Date(now)
  yesterday.setDate(now.getDate() - 1)
  var isYesterday = d.getFullYear() === yesterday.getFullYear()
    && d.getMonth() === yesterday.getMonth()
    && d.getDate() === yesterday.getDate()
  if (isYesterday) return "Yesterday"
  if (d.getFullYear() === now.getFullYear()) {
    return Qt.formatDate(d, "MMM d")
  }
  return Qt.formatDate(d, "MMM d, yyyy")
}

function messageText(msg) {
  var text = String((msg && msg.text) || "").replace(/\uFFFC/g, "").trim()
  if (text.length > 0) return text
  if (msg && msg.attachments && msg.attachments.length > 0) return "Attachment"
  return ""
}

function messagePreview(msg) {
  var text = messageText(msg)
  if (text.length > 80) return text.substring(0, 77) + "..."
  return text
}
