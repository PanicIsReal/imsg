.pragma library

function chatTitle(chat) {
  if (!chat) return "Chat"
  if (chat.contact_name && chat.contact_name.length > 0) return chat.contact_name
  if (chat.display_name && chat.display_name.length > 0) return chat.display_name
  if (chat.name && chat.name.length > 0) return chat.name
  if (chat.identifier) return chat.identifier
  return "Chat " + chat.id
}

function messagePreview(msg) {
  if (!msg) return ""
  var text = msg.text || ""
  if (text.length > 80) return text.substring(0, 77) + "..."
  return text
}
