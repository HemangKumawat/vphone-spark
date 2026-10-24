// vphone-linux overlay (new file): `fw aea-decrypt`, what macOS's `aea decrypt` does for a
// symmetric-key (profile 1) archive. linux-tools/aea maps `aea decrypt -i -o -key-value` onto it.
import ArgumentParser
import Foundation
import VPhoneCoreKit

struct VPhoneFirmwareAEADecryptCommand: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "aea-decrypt",
        abstract: "Decrypt an .aea archive (replaces `aea decrypt` on Linux)",
        discussion: """
        Without --key-value the key is fetched the way `fw aea-key` does it.
        """,
    )

    @Option(name: [.short, .customLong("input")], help: "The .aea archive", transform: URL.init(fileURLWithPath:))
    var input: URL

    @Option(name: [.short, .customLong("output")], help: "Where to write the plain file", transform: URL.init(fileURLWithPath:))
    var output: URL

    @Option(name: .customLong("key-value"), help: "base64:<key> or hex:<key>")
    var keyValue: String?

    func run() throws {
        let value = try keyValue ?? vphoneRunBlocking { try await VPhoneAEA.symmetricKey(of: input) }
        try VPhoneAEADecrypt.decrypt(input: input, output: output, key: VPhoneAEADecrypt.parseKey(value))
    }
}
