import assert from "node:assert/strict"
import fs from "node:fs"
import vm from "node:vm"

const filename = new URL("../plugin/js/Store.js", import.meta.url)
const source = fs.readFileSync(filename, "utf8").replace(/^\.pragma library\s*/, "")
const store = {}
vm.createContext(store)
vm.runInContext(source, store, { filename: filename.pathname })

const iso = n => `2026-09-06T12:00:0${n}.000Z`

let outbox = store.enqueue([], 7, "first", "c1", iso(1), 1)
outbox = store.enqueue(outbox, 7, "same text", "c2", iso(2), 2)
assert.deepEqual(Array.from(outbox, x => x.status), ["queued", "queued"])
assert.equal(store.nextQueued(outbox).client_id, "c1")

outbox = store.markOutbox(outbox, "c1", "sending", "", null)
assert.equal(store.nextQueued(outbox).client_id, "c2", "the queue can accept another send while one is running")

let state = { chats: [], messagesByChat: {}, outbox, openChatId: 7 }
let patch = store.applyEvent(state, { type: "event", topic: "sync.message", payload: {
  message: { id: 91, client_id: "c1", chat_id: 7, text: "first", is_from_me: true, created_at: iso(3) }
}})
assert.equal(patch.outbox[0].status, "sent", "a watch event may reconcile before the request response")
outbox = store.acknowledge(patch.outbox, "c1", { ok: true })
assert.equal(outbox[0].status, "sent", "an ID-less ACK cannot undo earlier event reconciliation")

outbox = store.acknowledge(outbox, "c2", { ok: true })
assert.equal(outbox[1].status, "unconfirmed", "an ID-less ACK remains visible and does not assume delivery")
outbox = store.retryAs(outbox, "c2", "c2-retry", iso(4))
assert.equal(outbox[1].status, "queued", "only an explicit retry requeues uncertain work")
assert.equal(outbox[1].client_id, "c2-retry", "retry uses a fresh identity so a late response cannot settle it")

let acknowledged = store.acknowledge(store.enqueue([], 7, "confirmed", "c3", iso(3), 3), "c3", { message: { id: 93 } })
assert.equal(store.messagesForChat({}, acknowledged, 7).length, 1, "an ACK does not make the optimistic bubble disappear before cache delivery")
patch = store.applyEvent({ chats: [], messagesByChat: {}, outbox: acknowledged, openChatId: 7 }, {
  type: "event", topic: "sync.message", payload: { message: { id: 93, chat_id: 7, text: "confirmed", is_from_me: false, created_at: iso(4) } }
})
assert.equal(patch.messagesByChat["7"][0].is_from_me, true, "a stale watch flag cannot demote a message confirmed as ours")
assert.equal(store.messagesForChat(patch.messagesByChat, patch.outbox, 7).length, 1, "server ID reconciliation replaces the local bubble")
assert.equal(store.failOutbox(patch.outbox, "c3", "failed", "late failure")[0].status, "sent", "a late process failure cannot demote a stream-confirmed send")
assert.equal(store.classifySendFailure({ code: "timeout", message: "timed out" }), "unconfirmed")
assert.equal(store.classifySendFailure({ code: "upstream_error", message: "rpc restarted" }), "unconfirmed")
assert.equal(store.classifySendFailure({ code: "upstream", message: "imsg rpc error" }), "unconfirmed")
assert.equal(store.classifySendFailure({ code: "link_down", message: "mac link is down" }), "unconfirmed")
assert.equal(store.classifySendFailure({ code: "transport", message: "write failed" }), "unconfirmed")
assert.equal(store.classifySendFailure({ code: "invalid_request", message: "empty text" }), "failed")

let byChat = store.mergeHistory({}, 7, [
  { id: 1, chat_id: 7, text: "old", created_at: iso(1) },
  { id: 2, chat_id: 7, text: "live version", created_at: iso(2) }
])
byChat = store.mergeHistory(byChat, 7, [
  { id: 1, chat_id: 7, text: "old", created_at: iso(1) },
  { id: 2, chat_id: 7, text: "history version", created_at: iso(2) }
], false)
assert.equal(byChat["7"].length, 2, "history merges by server ID")
assert.equal(byChat["7"][1].text, "live version", "older history cannot clobber a live record")
byChat = store.mergeHistory(byChat, 7, [{ id: 2, chat_id: 7, text: "edited", is_from_me: true, created_at: iso(2) }], true)
assert.equal(byChat["7"][1].text, "edited", "history refreshes existing rows when no live event raced the request")

const sameTextOutbox = store.enqueue([], 7, "duplicate", "local-a", iso(3), 3)
const display = store.messagesForChat({ "7": [{ id: 8, chat_id: 7, text: "duplicate", is_from_me: true, created_at: iso(4) }] }, sameTextOutbox, 7)
assert.equal(display.length, 2, "equal text alone never deduplicates an optimistic send")
assert.equal(store.messagesForChat({ "8": [{ id: 9, chat_id: 8, text: "elsewhere", created_at: iso(1) }] }, [], 7).length, 0)

patch = store.applyEvent({ chats: [] }, { type: "event", topic: "sync.chats", payload: { reason: "events_lagged", chats: [] } })
assert.equal(patch.resyncOpenChat, true)
patch = store.applyEvent({ chats: [] }, { type: "event", topic: "sync.resync", payload: { reason: "reconnect", chats: [] } })
assert.equal(patch.resyncOpenChat, true)
patch = store.applyEvent({ chats: [] }, { type: "event", topic: "sync.resync", payload: { reason: "db_generation", chats: [] } })
assert.equal(patch.resetGeneration, true)
assert.equal(store.applySnapshot({ db_generation: "new-db" }).databaseGeneration, "new-db")
assert.equal(store.shouldResetGeneration("", "baseline", false), false, "the first token establishes a baseline without invalidating a fast send")
assert.equal(store.shouldResetGeneration("baseline", "baseline", true), false, "a repeated reset notification for the same token is idempotent")
assert.equal(store.shouldResetGeneration("baseline", "replacement", false), true)
assert.equal(store.shouldResetGeneration("", "replacement", true), true, "an explicit reset still invalidates cached rows before the first snapshot")
assert.equal(store.applyEvent({}, { type: "event", topic: "sync.resync", payload: { db_generation: "new-db" } }).databaseGeneration, "new-db")
const generationOutbox = store.resetOutboxGeneration([
  { client_id: "done", status: "sent", server_id: 1 },
  { client_id: "maybe", status: "unconfirmed", server_id: 2 }
])
assert.equal(generationOutbox.length, 1, "confirmed entries from the old cache generation are removed")
assert.equal(generationOutbox[0].server_id, undefined, "unresolved sends detach old server IDs")
assert.equal(generationOutbox[0].retry_allowed, false, "old chat row IDs must not be reused for a retry")
assert.equal(store.retryAs(generationOutbox, "maybe", "retry", iso(4)), generationOutbox)

const listRows = []
const listModel = {
  get count() { return listRows.length },
  get(index) { return listRows[index] },
  insert(index, entry) { listRows.splice(index, 0, entry) },
  move(from, to) { listRows.splice(to, 0, listRows.splice(from, 1)[0]) },
  set(index, entry) { listRows[index] = entry },
  remove(index, count) { listRows.splice(index, count) }
}
store.syncListModel(listModel, [{ id: 2, text: "visible" }, { id: 3, text: "last" }])
const visibleRow = listRows[0]
store.syncListModel(listModel, [{ id: 1, text: "older" }, { id: 2, text: "visible" }, { id: 3, text: "last" }])
assert.equal(listRows[1], visibleRow, "prepending history preserves the existing view row")
store.syncListModel(listModel, [{ id: 1, text: "older" }, { id: 2, text: "edited" }, { id: 4, text: "new" }])
assert.deepEqual(Array.from(listRows, row => row.entry.id), [1, 2, 4])
assert.equal(listRows[1].entry.text, "edited")
store.syncListModel(listModel, [])
assert.equal(listRows.length, 0)

console.log("plugin realtime store tests passed")
