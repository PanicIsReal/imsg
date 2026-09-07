import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import vm from "node:vm"

const root = path.resolve(import.meta.dirname, "..")
const source = fs.readFileSync(path.join(root, "plugin/js/Store.js"), "utf8")
const context = vm.createContext({})
vm.runInContext(source.replace(/^\.pragma library$/m, ""), context)

const rows = [{ entry: { id: "2", text: "old" } }]
const model = {
  get count() { return rows.length },
  get(index) { return rows[index] },
  insert(index, row) { rows.splice(index, 0, row) },
  move(from, to) { rows.splice(to, 0, rows.splice(from, 1)[0]) },
  set(index, row) { rows[index] = row },
  remove(index, count) { rows.splice(index, count) },
}
context.syncListModel(model, [{ id: "1" }, { id: "2", text: "new" }])
assert.deepEqual(rows.map(row => row.entry.id), ["1", "2"])
assert.equal(rows[1].entry.text, "new")

let queue = context.enqueueOutgoing([], { id: "attempt-1", chat_id: "iMessage;+;chat-guid", text: "one", send_state: "queued" })
queue = context.enqueueOutgoing(queue, { id: "attempt-2", chat_id: "iMessage;+;chat-guid", text: "two", send_state: "queued" })
assert.equal(context.nextQueuedOutgoing(queue).id, "attempt-1")
queue = context.updateOutgoing(queue, "attempt-1", "sending", "")
assert.equal(context.nextQueuedOutgoing(queue).id, "attempt-2")
assert.equal(queue[0].chat_id, "iMessage;+;chat-guid")

queue = context.updateOutgoing(queue, "attempt-1", "unconfirmed", "connection closed")
queue = context.retryOutgoing(queue, "attempt-1", "attempt-3", "2026-09-06T00:00:00Z")
assert.equal(queue[0].id, "attempt-3")
assert.equal(queue[0].send_state, "queued")
assert.equal(queue[0].chat_id, "iMessage;+;chat-guid")
assert.equal(context.classifySendFailure({ code: "timeout" }, 0), "unconfirmed")
assert.equal(context.classifySendFailure({ code: "invalid_request", message: "bad recipient" }, 0), "failed")
assert.equal(context.classifySendFailure("", 1), "unconfirmed")

const eventPatch = context.applyMessage({ openChatId: "iMessage;+;chat-guid", chats: [], messages: [], outgoing: queue }, {
  message: { id: "server-1", client_id: "attempt-3", chat_id: "iMessage;+;chat-guid", is_from_me: true },
})
assert.deepEqual(Array.from(eventPatch.outgoing, row => row.id), ["attempt-2"])

queue = context.removeOutgoing(queue, "attempt-3")
assert.deepEqual(Array.from(queue, row => row.id), ["attempt-2"])

const service = fs.readFileSync(path.join(root, "plugin/Service.qml"), "utf8")
const panel = fs.readFileSync(path.join(root, "plugin/Panel.qml"), "utf8")
assert.match(service, /property var openChatId: ""/)
assert.doesNotMatch(service, /Number\(.*chatId/)
assert.match(panel, /enabled: Models\.hasId\(selectedChatId\) && imsg && root\.draftText\.trim\(\)\.length > 0/)
assert.match(panel, /Retry \(may duplicate\)/)
assert.match(panel, /model: threadModel/)
assert.match(panel, /Component\.onCompleted: Qt\.callLater\(root\.updateThreadModel\)/)
assert.match(panel, /onImsgChanged: Qt\.callLater\(root\.updateThreadModel\)/)
assert.doesNotMatch(service, /if \(sendProc\.restoreText\.length > 0\) root\.failedDraft = sendProc\.restoreText/)

const pickAttachmentBody = panel.match(/function pickAttachment\(\) \{([\s\S]*?)\n  \}/)[1]
let opened = false
vm.runInNewContext("(function() {" + pickAttachmentBody + "})()", {
  Models: { hasId: () => true }, selectedChatId: "chat", imsg: { sending: true },
  photoDialog: { open: () => { opened = true } },
})
assert.equal(opened, true, "Photo must open while another message is sending")
console.log("plugin-realtime.test.mjs ok")
