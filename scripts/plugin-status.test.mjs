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

assert.equal(typeof Client.notificationText, "function", "notificationText is missing")
assert.equal(Client.notificationText("September 11&12"), "September 11&amp;12")
assert.equal(Client.notificationText("<b>hi</b>"), "&lt;b&gt;hi&lt;/b&gt;")
assert.equal(Client.notificationText("a>b"), "a&gt;b")
assert.equal(Client.notificationText("ok"), "ok")
assert.notEqual(Client.notificationText("--exec").charAt(0), "-")
assert.notEqual(Client.notificationText("-g").charAt(0), "-")
assert.equal(Client.notificationText("x\0y"), "xy")

const toast = Client.notificationCommand("AT&T", "September 11&12", "99")
assert.equal(toast[toast.indexOf("-g") + 2], "AT&amp;T")
assert.equal(toast[toast.indexOf("-g") + 3], "September 11&amp;12")
assert.equal(toast[toast.indexOf("--exec") + 1], "omarchy-shell")
assert.ok(!toast.includes("<b>"))
const inject = Client.notificationCommand("x", "<a href=\"javascript:alert(1)\">z</a>", "1")
assert.ok(inject.some((part) => String(part).indexOf("&lt;a") !== -1))
assert.ok(!inject.some((part) => String(part).indexOf("<a href") !== -1))

const failed = Store.finishOutgoing(
  [{ id: "pending-1", chat_id: "c", text: "hi", send_state: "sending" }],
  "pending-1",
  false,
)
assert.equal(failed[0].send_state, "failed")
assert.equal(Store.discardOutgoing(failed, "pending-1").length, 0)
assert.equal(Store.markOutgoingSending(failed, "pending-1")[0].send_state, "sending")
assert.equal(Store.findOutgoing(failed, "pending-1").text, "hi")

console.log("plugin-status.test.mjs ok")
