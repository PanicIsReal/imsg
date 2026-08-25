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
