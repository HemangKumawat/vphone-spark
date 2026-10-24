// Linux stand-in: vphone-cli only builds file-type lists for macOS open panels.
public struct UTType: Hashable, Sendable {
    public let filenameExtension: String
    public init?(filenameExtension: String) { self.filenameExtension = filenameExtension }
}
