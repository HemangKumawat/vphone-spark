// Linux stand-in for apple/swift-system's `SystemPackage` module, named `System` because
// vphone-cli 2.12's VPhoneOTAArchive imports it under that name.
//
// That file uses exactly two symbols from it: `Errno.notSupported` (thrown by the unsupported
// seek/write paths of its segment byte stream) and the `FileDescriptor.SeekOrigin` parameter
// type of its `seek` method. Everything else in VPhoneOTAArchive is stubbed by the AppleArchive
// stand-in, so those two are all this module needs — and it keeps the build from depending on a
// network fetch for a package we barely touch.
import Glibc

public enum FileDescriptor {
    public enum SeekOrigin: Sendable {
        case start
        case current
        case end
    }
}

public struct Errno: Error, Sendable, Equatable {
    public let rawValue: Int32

    public init(rawValue: Int32) {
        self.rawValue = rawValue
    }

    public static let notSupported = Errno(rawValue: ENOTSUP)
}

public struct FilePath: Sendable, Hashable {
    public let string: String

    public init(_ string: String) {
        self.string = string
    }
}

public struct FilePermissions: OptionSet, Sendable {
    public let rawValue: UInt32

    public init(rawValue: UInt32) {
        self.rawValue = rawValue
    }
}
