#!/usr/bin/env node
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import vm from "node:vm"
import { fileURLToPath } from "node:url"

const here = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(here, "..")
const pluginRoot = process.env.IMSG_PLUGIN_ROOT || path.join(repoRoot, "plugin")

function loadLibrary(rel) {
  const file = path.join(pluginRoot, rel)
  const src = fs.readFileSync(file, "utf8").replace(/^\.pragma library\s*/m, "")
  const ctx = { console }
  vm.createContext(ctx)
  vm.runInContext(src, ctx, { filename: file })
  return ctx
}

function walkFiles(dir, acc) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const next = path.join(dir, entry.name)
    if (entry.isDirectory()) walkFiles(next, acc)
    else acc.push(next)
  }
  return acc
}

const banned =
  /Grant Full Disk|Full Disk Access|mac-locked|needs-fda|Messages is locked|Mac Messages database is locked/i
const uiFiles = walkFiles(pluginRoot, []).filter((file) => /\.(qml|js)$/.test(file))
const hits = []
for (const file of uiFiles) {
  const text = fs.readFileSync(file, "utf8")
  const lines = text.split("\n")
  for (let i = 0; i < lines.length; i++) {
    if (banned.test(lines[i])) hits.push(`${file}:${i + 1}:${lines[i].trim()}`)
  }
}
assert.equal(hits.length, 0, `FDA copy still in plugin UI:\n${hits.join("\n")}`)

const Store = loadLibrary("js/Store.js")
assert.equal(typeof Store.linkState, "function", "Store.linkState is missing")
assert.equal(typeof Store.setupGuide, "function", "Store.setupGuide is missing")

const liveWorking = {
  connected: true,
  cacheReady: true,
  statusKnown: true,
  bridgeConnected: true,
  databaseReady: false,
  passwordSet: true,
  contacts: "granted",
  namesVisible: true,
}

assert.equal(Store.linkState(liveWorking), "live")
assert.notEqual(Store.linkState(liveWorking), "mac-locked")

for (const databaseReady of [true, false]) {
  for (const cacheReady of [true, false]) {
    const state = Store.linkState({
      connected: true,
      cacheReady,
      statusKnown: true,
      bridgeConnected: true,
      databaseReady,
    })
    assert.equal(
      state,
      "live",
      `linkState=${state} with cacheReady=${cacheReady} databaseReady=${databaseReady}`,
    )
  }
}

function noFdaCopy(guide, label) {
  const blob = JSON.stringify(guide)
  assert.notEqual(guide.phase, "needs-fda", `${label} phase=${guide.phase}`)
  assert.equal(banned.test(blob), false, `${label} still has FDA copy: ${blob}`)
}

const ready = Store.setupGuide(liveWorking)
assert.equal(ready.phase, "ready")
noFdaCopy(ready, "ready with chats while database_ready is false")

const loading = Store.setupGuide({
  connected: true,
  cacheReady: false,
  statusKnown: true,
  bridgeConnected: true,
  databaseReady: false,
  passwordSet: true,
  contacts: "unknown",
  namesVisible: false,
})
assert.equal(loading.phase, "loading")
noFdaCopy(loading, "loading conversations")

const Client = loadLibrary("js/ImsgClient.js")
assert.notEqual(Client.friendlyError("database_unavailable"), "Mac Messages database is locked")
assert.notEqual(Client.friendlyError("Full Disk Access required"), "Mac Messages database is locked")
assert.notEqual(Client.friendlyError("Database unavailable"), "Mac Messages database is locked")

assert.equal(Client.clampWebhookPort(8080), 8080)
assert.equal(Client.clampWebhookPort("18792"), 18792)
assert.equal(Client.clampWebhookPort(0), 18792)
assert.equal(Client.clampWebhookPort("nope"), 18792)
assert.equal(Client.clampWebhookPort(65536), 18792)
assert.equal(Client.clampWebhookPort(-3), 18792)

const serveScript = Client.webhookServeScript(8080)
assert.equal(serveScript.includes("tailscale serve --bg --yes localhost:8080"), true)
assert.equal(serveScript.includes("tailscale serve status"), true)
assert.equal(/funnel/i.test(serveScript), false)
const injected = Client.webhookServeScript("8080; rm -rf /")
assert.equal(injected.includes("rm"), false)
assert.equal(injected.includes("localhost:8080"), true)

const launch = Client.webhookServeLaunchCommand(18792)
assert.equal(launch.startsWith("omarchy-launch-floating-terminal-with-presentation '"), true)
assert.equal(launch.includes("funnel"), false)
assert.equal(launch.includes("localhost:18792"), true)

const resetLaunch = Client.webhookServeResetLaunchCommand()
assert.equal(resetLaunch.includes("tailscale serve reset"), true)
assert.equal(/funnel/i.test(resetLaunch), false)
assert.equal(Client.webhookServeIsActive(null), false)
assert.equal(Client.webhookServeIsActive({}), false)
assert.equal(Client.webhookServeIsActive({ Web: {}, TCP: {} }), false)
assert.equal(
  Client.webhookServeIsActive({
    Web: { "host:443": { Handlers: { "/": { Proxy: "http://localhost:18792" } } } },
  }),
  true,
)
assert.equal(Client.webhookServeIsActive({ TCP: { "443": { HTTPS: true } } }), true)

function guidePhase(input) {
  return Store.webhookGuide(input).phase
}

assert.equal(guidePhase({}), "needs-enable")
assert.equal(Store.webhookGuide({}).actionKind, "enable")
assert.equal(Store.webhookGuide({ enabled: true }).phase, "waiting")
assert.equal(
  guidePhase({ enabled: true, listening: true }),
  "needs-serve",
)
assert.equal(
  Store.webhookGuide({ enabled: true, listening: true }).actionKind,
  "serve",
)
assert.equal(
  guidePhase({ enabled: true, listening: true, serveOffered: true, session: "down" }),
  "needs-live",
)
assert.equal(
  Store.webhookGuide({
    enabled: true,
    listening: true,
    serveOffered: true,
    session: "live",
  }).actionKind,
  "register",
)
assert.equal(
  guidePhase({
    enabled: true,
    listening: true,
    registered: true,
    session: "live",
  }),
  "ready",
)
assert.equal(
  Store.webhookGuide({
    enabled: true,
    listening: true,
    registered: true,
  }).actionKind,
  "",
)
assert.notEqual(
  Store.webhookGuide({ enabled: true, listening: true }).actionKind,
  "register",
)

console.log("plugin-status.test.mjs ok")

// Exercise the shipped QML functions with mocked Process objects.
const serviceSource = fs.readFileSync(path.join(pluginRoot, "Service.qml"), "utf8")
const panelSource = fs.readFileSync(path.join(pluginRoot, "Panel.qml"), "utf8")
function qmlFunction(source, name, context) {
  const match = source.match(new RegExp("  function " + name + "\\(([^)]*)\\) \\{([\\s\\S]*?)\\n  \\}"))
  assert.ok(match, name + " exists")
  vm.createContext(context)
  return vm.runInContext("(function(" + match[1] + ") {" + match[2] + "})", context)
}
const mockRoot = { pendingNotify: [], openChatId: "B" }
const mockHistory = { running: true, beforeCursor: "original", chatId: "A" }
const load = qmlFunction(serviceSource, "loadMessages", {
  root: mockRoot, historyProc: mockHistory, startRequest() { return true }
})
load("B", null)
assert.equal(mockHistory.chatId, "A", "in-flight history retains its chat")
assert.equal(mockHistory.beforeCursor, "original", "in-flight cursor is immutable")
assert.equal(mockHistory.pendingRequest.chat_id, "B", "latest requested chat is queued")
mockHistory.running = false
load("B", null)
assert.equal(mockHistory.chatId, "B")
const notify = qmlFunction(serviceSource, "notifyInbound", {
  root: mockRoot, notifyProc: { running: true }, ImsgClient: Client
})
notify("Ada", "one", "1")
notify("Ada", "two", "1")
assert.equal(mockRoot.pendingNotify.length, 2, "bursts retain every pending alert")
const active = { imsg: { openChatId: "A", markRead() { throw Error("hidden chat marked read") } },
  opened: false, settingsVisible: false, selectedChatId: "A" }
qmlFunction(panelSource, "updateActiveChat", active)()
assert.equal(active.imsg.openChatId, "", "closed panel does not suppress notifications")
active.opened = true
active.settingsVisible = true
qmlFunction(panelSource, "updateActiveChat", active)()
assert.equal(active.imsg.openChatId, "", "settings does not suppress notifications")
const merged = Store.mergeMessages(
  [{id: "2", created_at: "2026-01-02", text: "old"}],
  [{id: "1", created_at: "2026-01-01"}, {id: "2", created_at: "2026-01-02", text: "new"}])
assert.equal(merged.length, 2)
assert.equal(merged[0].id, "1", "late events are ordered chronologically")
assert.equal(merged[1].text, "new", "newer live payload wins over stale history")
assert.equal(Store.applyMessage({openChatId: "", chats: []}, {
  is_new: true, message: {id: "9", chat_id: "A", is_from_me: false, attachments: [{}]}
}).notify.preview, "Attachment")
console.log("plugin realtime regression tests ok")

const malicious = Client.notificationCommand("--app-name", "--exec", "42")
assert.equal(malicious[0], "busctl")
assert.equal(malicious[12], "--app-name", "sender stays a typed string")
assert.equal(malicious[13], "--exec", "body stays a typed string")
assert.deepEqual(JSON.parse(malicious[24]), ["omarchy-shell", "io.github.panic.imessage", "openChat", "42"])
assert.equal(Client.notificationCommand("Ada", "<b>hi</b>", "42")[13], "&lt;b&gt;hi&lt;/b&gt;")
console.log("notification argument regression tests ok")
