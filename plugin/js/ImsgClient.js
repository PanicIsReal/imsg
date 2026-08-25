.pragma library

function parseResponse(stdout) {
  if (!stdout || stdout.length === 0) return null
  try {
    return JSON.parse(stdout.trim())
  } catch (e) {
    return null
  }
}

function request(method, params) {
  var paramsJson = JSON.stringify(params || {})
  var cmd = "imsg-sync request " + method + " --params '" + paramsJson.replace(/'/g, "'\\''") + "'"
  return cmd
}
