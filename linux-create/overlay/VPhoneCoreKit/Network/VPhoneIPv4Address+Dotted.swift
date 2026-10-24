// vphone-linux overlay: the dotted-quad parser from VPhoneUDPForwarder.swift (whose forwarder is
// macOS-only and left out).
import Foundation

public extension VPhoneIPv4Address {
    /// Parse `a.b.c.d`, or nil for anything else (including IPv6, which this
    /// stack deliberately does not carry).
    init?(dotted: String) {
        let parts = dotted.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 4 else { return nil }
        var octets: [UInt8] = []
        for part in parts {
            guard let value = UInt8(part) else { return nil }
            octets.append(value)
        }
        self.init(octets[0], octets[1], octets[2], octets[3])
    }
}
