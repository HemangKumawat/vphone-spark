// Linux stand-in for Apple's <AppleArchive> framework.
//
// vphone-cli 2.12's VPhoneOTAArchive reads full OTA payloads with AppleArchive (an Apple
// proprietary archive format that libarchive does not read). The OTA workflow is macOS-only for
// now; the Linux build needs the module to compile, so this stand-in declares exactly the API
// surface that file uses and refuses to open anything at runtime.
//
// Supported on Linux: nothing. `decodeStream` returns nil, so any attempt to read an OTA fails
// with the caller's own "not an OTA" error instead of pretending to work.
import Foundation
import System

public struct ArchiveHeader: Sendable {
    /// A header field name, e.g. "TYP", "PAT", "DAT".
    public struct FieldKey: Hashable, Sendable, CustomStringConvertible {
        public let name: String

        public init(_ name: String) {
            self.name = name
        }

        public var description: String { name }
    }

    /// A decoded field. The three shapes VPhoneOTAArchive matches on.
    public enum Field: Sendable {
        case uint(FieldKey, UInt64)
        case string(FieldKey, String)
        case blob(FieldKey, UInt64, UInt32)
    }

    public init() {}

    public func field(forKey key: FieldKey) -> Field? {
        _ = key
        return nil
    }
}

public final class ArchiveStream: @unchecked Sendable {
    public init() {}

    /// Unsupported on Linux: callers treat nil as "not an Apple Archive".
    public static func decodeStream(readingFrom data: Data) -> ArchiveStream? {
        _ = data
        return nil
    }

    public static func decodeStream(readingFrom stream: ArchiveByteStream) -> ArchiveStream? {
        _ = stream
        return nil
    }

    public func readHeader() throws -> ArchiveHeader? {
        nil
    }

    public func close() throws {}

    public func readBlob(key: ArchiveHeader.FieldKey, into buffer: UnsafeMutableRawBufferPointer) throws {
        _ = key
        _ = buffer
        throw Errno.notSupported
    }
}

/// The byte-stream protocol VPhoneOTAArchive's segment stream conforms to.
public protocol ArchiveByteStreamProtocol {
    func read(into buffer: UnsafeMutableRawBufferPointer) throws -> Int
    func read(into buffer: UnsafeMutableRawBufferPointer, atOffset offset: Int64) throws -> Int
    func write(from buffer: UnsafeRawBufferPointer) throws -> Int
    func write(from buffer: UnsafeRawBufferPointer, atOffset offset: Int64) throws -> Int
    func seek(toOffset offset: Int64, relativeTo origin: FileDescriptor.SeekOrigin) throws -> Int64
    func cancel()
    func close() throws
}

/// A byte stream over a file or a transform. Only the constructors VPhoneOTAArchive calls are
/// declared; each returns nil on Linux, so the caller's own "unreadable"/"not an OTA" error wins.
public final class ArchiveByteStream: @unchecked Sendable {
    public enum AccessMode: Sendable {
        case readOnly
        case writeOnly
        case readWrite
    }

    public struct Options: OptionSet, Sendable {
        public let rawValue: Int

        public init(rawValue: Int) {
            self.rawValue = rawValue
        }
    }

    public static func customStream<T: ArchiveByteStreamProtocol>(instance: T) -> ArchiveByteStream? {
        _ = instance
        return nil
    }

    public static func fileStream(
        path: FilePath, mode: AccessMode, options: Options, permissions: FilePermissions,
    ) -> ArchiveByteStream? {
        _ = path
        _ = mode
        _ = options
        _ = permissions
        return nil
    }

    public static func decryptionStream(
        readingFrom stream: ArchiveByteStream, encryptionContext: ArchiveEncryptionContext,
    ) -> ArchiveByteStream? {
        _ = stream
        _ = encryptionContext
        return nil
    }

    public static func decompressionStream(readingFrom stream: ArchiveByteStream) -> ArchiveByteStream? {
        _ = stream
        return nil
    }

    public func close() throws {}
}

public final class ArchiveEncryptionContext {
    public init?(from stream: ArchiveByteStream) {
        _ = stream
        return nil
    }

    public func setSymmetricKey<Key>(_ key: Key) throws {
        _ = key
    }
}
