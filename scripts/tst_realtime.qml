import QtQuick
import QtTest
import "../plugin/js/Store.js" as Store
TestCase {
 name: "RealtimeModel"
 ListModel { id: messages; dynamicRoles: true }
 function test_reconcile() {
  Store.syncListModel(messages, [{id: "a", text: "first", attachments: []}])
  compare(messages.count, 1)
  compare(messages.get(0).entry.text, "first")
  Store.syncListModel(messages, [{id: "b", text: "older"}, {id: "a", text: "updated", attachments: [{name: "photo"}]}])
  compare(messages.count, 2)
  compare(messages.get(1).entry.text, "updated")
  compare(messages.get(1).entry.attachments[0].name, "photo")
  Store.syncListModel(messages, [{id: "a", text: "updated", attachments: [{name: "photo"}]}])
  compare(messages.count, 1)
  compare(messages.get(0).entry.id, "a")
 }
}
