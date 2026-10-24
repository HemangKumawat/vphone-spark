// vphone-linux overlay (new file): Apple Encrypted Archive decryption without macOS's aea tool.
//
// Profile 1 (symmetric key, unsigned), the profile of every IPSW `.dmg.aea`. Layout and key
// schedule follow the reverse-engineered format (blacktop/ipsw pkg/aea):
//
//   "AEA1" | u32 profile (low 24 bits) + scrypt strength | u32 auth data size | auth data
//   32-byte main salt
//   root header: HMAC(32) | encrypted root header (48) | first cluster's HMAC(32)
//   clusters: encrypted segment headers | next cluster's HMAC | segment HMACs | segment data...
//   padding
//
// Keys come from HKDF-SHA256: the main key from the archive key ("AEA_AMK" + profile word, salted
// with the main salt), and from it the root header key ("AEA_RHEK"), each cluster's key
// ("AEA_CK" + index), that cluster's header key ("AEA_CHEK") and each segment's key ("AEA_SK" +
// index). A header key is 80 bytes: HMAC key, AES-256 key, CTR IV. Every HMAC is
// HMAC-SHA256(salt | data | u64 salt length). Each segment is AES-CTR encrypted, then optionally
// compressed (the root header says how) and checksummed.

import Crypto
import _CryptoExtras
import Foundation
import VLCompression

public enum VPhoneAEADecrypt {
    public enum Error: Swift.Error, CustomStringConvertible {
        case notAEA
        case unsupportedProfile(UInt32)
        case badKey
        case truncated
        case authentication(String)
        case unsupportedCompression(UInt8)
        case unsupportedChecksum(UInt8)
        case decompression(Int, Int)
        case checksum(Int, Int)
        case size(UInt64, UInt64)

        public var description: String {
            switch self {
            case .notAEA: "not an AEA1 archive"
            case let .unsupportedProfile(p): "AEA profile \(p) is not supported (only 1, symmetric key)"
            case .badKey: "the key is not base64:<32 bytes>"
            case .truncated: "the archive is truncated"
            case let .authentication(what): "\(what) failed authentication (wrong key or damaged archive)"
            case let .unsupportedCompression(c): "segment compression '\(Character(UnicodeScalar(c)))' is not supported"
            case let .unsupportedChecksum(c): "segment checksum type \(c) is not supported"
            case let .decompression(c, s): "cluster \(c) segment \(s) did not decompress"
            case let .checksum(c, s): "cluster \(c) segment \(s) checksum mismatch"
            case let .size(got, want): "decrypted \(got) bytes, the archive says \(want)"
            }
        }
    }

    private struct HeaderKey {
        let mac: SymmetricKey
        let key: SymmetricKey
        let iv: Data

        init(_ bytes: Data) {
            mac = SymmetricKey(data: bytes.prefix(32))
            key = SymmetricKey(data: bytes.dropFirst(32).prefix(32))
            iv = Data(bytes.dropFirst(64).prefix(16))
        }

        func decrypt(_ data: Data) throws -> Data {
            try AES._CTR.decrypt(data, using: key, nonce: AES._CTR.Nonce(nonceBytes: iv))
        }

        func verify(_ data: Data, salt: Data, expected: Data, what: @autoclosure () -> String) throws {
            var hmac = HMAC<SHA256>(key: mac)
            hmac.update(data: salt)
            hmac.update(data: data)
            var length = UInt64(salt.count).littleEndian
            hmac.update(data: Data(bytes: &length, count: 8))
            guard Data(hmac.finalize()) == expected else { throw Error.authentication(what()) }
        }
    }

    private static func derive(_ key: SymmetricKey, salt: Data = Data(), info: Data, count: Int) -> Data {
        HKDF<SHA256>.deriveKey(inputKeyMaterial: key, salt: salt, info: info, outputByteCount: count)
            .withUnsafeBytes { Data($0) }
    }

    private static func info(_ label: String, _ value: UInt32? = nil) -> Data {
        var data = Data(label.utf8)
        if var value = value?.littleEndian { data.append(Data(bytes: &value, count: 4)) }
        return data
    }

    /// The key as `aea decrypt -key-value` takes it: `base64:<key>` (or `hex:<key>`).
    public static func parseKey(_ value: String) throws -> SymmetricKey {
        let bytes: Data?
        if value.hasPrefix("base64:") {
            bytes = Data(base64Encoded: String(value.dropFirst(7)))
        } else if value.hasPrefix("hex:") {
            let hex = Array(value.dropFirst(4).utf8)
            var out = Data()
            var ok = hex.count % 2 == 0
            var i = 0
            while ok, i < hex.count {
                if let byte = UInt8(String(decoding: hex[i ..< i + 2], as: UTF8.self), radix: 16) { out.append(byte) } else { ok = false }
                i += 2
            }
            bytes = ok ? out : nil
        } else {
            bytes = Data(base64Encoded: value)
        }
        guard let bytes, bytes.count == 32 else { throw Error.badKey }
        return SymmetricKey(data: bytes)
    }

    /// Decrypt `input` into `output` (created or truncated).
    public static func decrypt(input: URL, output: URL, key archiveKey: SymmetricKey) throws {
        let reader = try FileHandle(forReadingFrom: input)
        defer { try? reader.close() }
        func read(_ count: Int) throws -> Data {
            guard count > 0 else { return Data() }
            guard let data = try reader.read(upToCount: count), data.count == count else { throw Error.truncated }
            return data
        }
        func u32(_ data: Data, _ offset: Int) -> UInt32 {
            data.withUnsafeBytes { UInt32(littleEndian: $0.loadUnaligned(fromByteOffset: offset, as: UInt32.self)) }
        }
        func u64(_ data: Data, _ offset: Int) -> UInt64 {
            data.withUnsafeBytes { UInt64(littleEndian: $0.loadUnaligned(fromByteOffset: offset, as: UInt64.self)) }
        }

        let header = try read(12)
        guard header.prefix(4) == Data("AEA1".utf8) else { throw Error.notAEA }
        let profileWord = u32(header, 4)
        guard profileWord & 0xFFFFFF == 1 else { throw Error.unsupportedProfile(profileWord & 0xFFFFFF) }
        let authData = try read(Int(u32(header, 8)))
        let mainSalt = try read(32)
        let mainKey = SymmetricKey(data: derive(archiveKey, salt: mainSalt, info: info("AEA_AMK", profileWord), count: 32))

        let rootMAC = try read(32)
        let encryptedRoot = try read(48)
        var clusterMAC = try read(32)
        let rootKey = HeaderKey(derive(mainKey, info: info("AEA_RHEK"), count: 80))
        try rootKey.verify(encryptedRoot, salt: clusterMAC + authData, expected: rootMAC, what: "the root header")
        let root = try rootKey.decrypt(encryptedRoot)
        let fileSize = u64(root, 0)
        let segmentSize = Int(u32(root, 16))
        let segmentsPerCluster = Int(u32(root, 20))
        let compression = root[root.startIndex + 24]
        let checksumType = root[root.startIndex + 25]
        let checksumSize: Int
        switch checksumType {
        case 0: checksumSize = 0
        case 1: checksumSize = 8
        case 2: checksumSize = 32
        default: throw Error.unsupportedChecksum(checksumType)
        }
        let algorithm: compression_algorithm?
        switch compression {
        case UInt8(ascii: "-"): algorithm = nil
        case UInt8(ascii: "e"): algorithm = COMPRESSION_LZFSE
        case UInt8(ascii: "z"): algorithm = COMPRESSION_ZLIB
        case UInt8(ascii: "x"): algorithm = COMPRESSION_LZMA
        case UInt8(ascii: "4"): algorithm = COMPRESSION_LZ4
        default: throw Error.unsupportedCompression(compression)
        }

        FileManager.default.createFile(atPath: output.path, contents: nil)
        let writer = try FileHandle(forWritingTo: output)
        defer { try? writer.close() }
        try writer.truncate(atOffset: 0)

        let segmentHeaderSize = 8 + checksumSize
        var produced: UInt64 = 0
        var cluster = 0
        var decompressed = [UInt8](repeating: 0, count: segmentSize)
        while produced < fileSize {
            let clusterKey = SymmetricKey(data: derive(mainKey, info: info("AEA_CK", UInt32(cluster)), count: 32))
            let clusterHeaderKey = HeaderKey(derive(clusterKey, info: info("AEA_CHEK"), count: 80))
            let encryptedHeaders = try read(segmentHeaderSize * segmentsPerCluster)
            let nextClusterMAC = try read(32)
            let segmentMACs = try read(32 * segmentsPerCluster)
            try clusterHeaderKey.verify(
                encryptedHeaders, salt: nextClusterMAC + segmentMACs, expected: clusterMAC,
                what: "cluster \(cluster)'s headers",
            )
            let headers = try clusterHeaderKey.decrypt(encryptedHeaders)
            for segment in 0 ..< segmentsPerCluster {
                let h = segment * segmentHeaderSize
                let plainSize = Int(u32(headers, h))
                let storedSize = Int(u32(headers, h + 4))
                guard plainSize > 0 else { continue }
                let encrypted = try read(storedSize)
                let segmentKey = HeaderKey(derive(clusterKey, info: info("AEA_SK", UInt32(segment)), count: 80))
                let mac = segmentMACs.dropFirst(32 * segment).prefix(32)
                try segmentKey.verify(encrypted, salt: Data(), expected: Data(mac), what: "cluster \(cluster) segment \(segment)")
                let stored = try segmentKey.decrypt(encrypted)
                var plain: Data
                if plainSize == storedSize || algorithm == nil {
                    plain = stored
                } else {
                    if decompressed.count < plainSize { decompressed = [UInt8](repeating: 0, count: plainSize) }
                    let n = stored.withUnsafeBytes { src in
                        decompressed.withUnsafeMutableBufferPointer { dst in
                            compression_decode_buffer(
                                dst.baseAddress!, plainSize,
                                src.bindMemory(to: UInt8.self).baseAddress!, storedSize, nil, algorithm!,
                            )
                        }
                    }
                    guard n == plainSize else { throw Error.decompression(cluster, segment) }
                    plain = Data(decompressed[0 ..< plainSize])
                }
                let expected = headers.dropFirst(h + 8).prefix(checksumSize)
                switch checksumType {
                case 2:
                    guard Data(SHA256.hash(data: plain)) == Data(expected) else { throw Error.checksum(cluster, segment) }
                case 1:
                    // MurmurHash64A, seed 0xE2236FDC26A5F6D2.
                    guard murmur64(plain) == u64(Data(expected), 0) else { throw Error.checksum(cluster, segment) }
                default:
                    break
                }
                try writer.seek(toOffset: UInt64(cluster * segmentsPerCluster + segment) * UInt64(segmentSize))
                try writer.write(contentsOf: plain)
                produced += UInt64(plainSize)
            }
            clusterMAC = nextClusterMAC
            cluster += 1
        }
        let size = try writer.seekToEnd()
        guard size == fileSize else { throw Error.size(size, fileSize) }
    }

    /// MurmurHash64A (Austin Appleby), the checksum AEA's "murmur" type uses.
    static func murmur64(_ data: Data, seed: UInt64 = 0xE223_6FDC_26A5_F6D2) -> UInt64 {
        let m: UInt64 = 0xC6A4_A793_5BD1_E995
        let r: UInt64 = 47
        var h = seed ^ (UInt64(data.count) &* m)
        data.withUnsafeBytes { raw in
            let n = data.count / 8
            for i in 0 ..< n {
                var k = UInt64(littleEndian: raw.loadUnaligned(fromByteOffset: i * 8, as: UInt64.self))
                k = k &* m; k ^= k >> r; k = k &* m
                h ^= k; h = h &* m
            }
            let tail = data.count & 7
            if tail > 0 {
                var t: UInt64 = 0
                for j in (0 ..< tail).reversed() { t = (t << 8) | UInt64(raw[n * 8 + j]) }
                h ^= t; h = h &* m
            }
        }
        h ^= h >> r; h = h &* m; h ^= h >> r
        return h
    }
}
