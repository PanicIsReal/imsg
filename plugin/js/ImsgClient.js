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

function command(script, method, params) {
  return ["/usr/bin/python3", script, method, JSON.stringify(params || {})]
}

function streamCommand(script) {
  return ["/usr/bin/python3", script]
}

function flag(value) {
  return value === true || value === 1 || value === "true"
}

function friendlyError(err) {
  var s = String(err || "")
  if (s.length === 0) return ""
  if (s === "database_unavailable" || s.indexOf("Database unavailable") !== -1 || s.indexOf("Full Disk Access") !== -1) {
    return "Mac Messages database is locked"
  }
  if (s === "sync_down" || s.indexOf("request failed") !== -1) {
    return "Local sync is down"
  }
  if (s.length > 140) return s.substring(0, 137) + "..."
  return s
}
