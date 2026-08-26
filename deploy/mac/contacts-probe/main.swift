import Contacts
import Foundation

let status = CNContactStore.authorizationStatus(for: .contacts)
let label: String
switch status {
case .authorized:
    label = "authorized"
case .denied:
    label = "denied"
case .restricted:
    label = "restricted"
case .notDetermined:
    label = "not_determined"
case .limited:
    label = "authorized"
@unknown default:
    label = "not_determined"
}

let payload = ["status": label]
let data = try JSONSerialization.data(withJSONObject: payload)
if let line = String(data: data, encoding: .utf8) {
    FileHandle.standardOutput.write(Data((line + "\n").utf8))
}
