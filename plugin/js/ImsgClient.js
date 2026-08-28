.pragma library

function parseResponse(stdout) {
  if (!stdout || stdout.length === 0) return null
  try {
    return JSON.parse(stdout.trim())
  } catch (e) {
    return null
  }
}

function scriptPath(resolvedUrl) {
  var s = String(resolvedUrl || "")
  if (s.indexOf("file://") === 0) s = s.substring(7)
  return decodeURIComponent(s)
}

function command(script, method) {
  return ["/usr/bin/python3", script, method]
}

function paramsPayload(params) {
  return JSON.stringify(params || {})
}

function streamCommand(script) {
  return ["/usr/bin/python3", script]
}

function notificationCommand(sender, body, chatId) {
  return [
    "omarchy-notification-send",
    "--app-name", "iMessage",
    "--urgency", "normal",
    "-g", "󰍩",
    String(sender || "iMessage"),
    String(body || ""),
    "--exec",
    "omarchy-shell",
    "io.github.panic.imessage",
    "openChat",
    String(chatId || "0")
  ]
}

function flag(value) {
  return value === true || value === 1 || value === "true"
}

function clampWebhookPort(port) {
  var n = parseInt(port, 10)
  if (!isFinite(n) || n < 1 || n > 65535) return 18792
  return n
}

function webhookServeScript(port) {
  var target = "localhost:" + clampWebhookPort(port)
  return "echo Publishing " + target + " on your tailnet...; tailscale serve --bg --yes " + target + "; echo; tailscale serve status"
}

function webhookServeLaunchCommand(port) {
  var inner = webhookServeScript(port)
  return "omarchy-launch-floating-terminal-with-presentation '" + String(inner).replace(/'/g, "'\\''") + "'"
}

function webhookServeResetScript() {
  return "echo Removing Tailscale Serve...; tailscale serve reset; echo; tailscale serve status"
}

function webhookServeResetLaunchCommand() {
  var inner = webhookServeResetScript()
  return "omarchy-launch-floating-terminal-with-presentation '" + String(inner).replace(/'/g, "'\\''") + "'"
}

function webhookServeIsActive(status) {
  if (!status || typeof status !== "object") return false
  var web = status.Web
  if (web && typeof web === "object") {
    for (var key in web) {
      if (Object.prototype.hasOwnProperty.call(web, key)) return true
    }
  }
  var tcp = status.TCP
  if (tcp && typeof tcp === "object") {
    for (var key in tcp) {
      if (Object.prototype.hasOwnProperty.call(tcp, key)) return true
    }
  }
  return false
}

function friendlyError(err) {
  var s = String(err || "")
  if (s.length === 0) return ""
  if (s === "sync_down" || s.indexOf("request failed") !== -1) {
    return "Local sync is down"
  }
  if (s.length > 140) return s.substring(0, 137) + "..."
  return s
}
