// vphone-linux: Linux version of libimg4-spm 0.1.1's Crypto.swift. CommonCrypto's CCCrypt and
// Security.framework's RSA verification are replaced with OpenSSL (Shims/Img4OpenSSL); the
// functions, inputs and results are the same.
import Foundation
import Img4OpenSSL

// MARK: - AES-256-CBC Decryption

enum Img4Crypto {
    /// Decrypt data using AES-256-CBC.
    /// - Parameters:
    ///   - data: Ciphertext bytes.
    ///   - ivHex: 32-character hex string (128-bit IV).
    ///   - keyHex: 64-character hex string (256-bit key).
    /// - Returns: Decrypted plaintext.
    static func aesDecrypt(_ data: Data, ivHex: String, keyHex: String) throws -> Data {
        let iv = try hexToBytes(ivHex)
        let key = try hexToBytes(keyHex)

        guard iv.count == 16 else {
            throw Img4Error.operationFailed("IV must be 16 bytes (32 hex chars)")
        }
        guard key.count == 32 else {
            throw Img4Error.operationFailed("Key must be 32 bytes (64 hex chars)")
        }

        var out = [UInt8](repeating: 0, count: data.count + 16)
        var outLength = 0
        let status = data.withUnsafeBytes { input in
            img4_aes256_cbc_decrypt(key, iv, input.bindMemory(to: UInt8.self).baseAddress, data.count, &out, &outLength)
        }
        guard status == 0 else {
            throw Img4Error.operationFailed("AES decryption failed")
        }
        return Data(out.prefix(outLength))
    }

    /// Parse a hex string into bytes.
    static func hexToBytes(_ hex: String) throws -> [UInt8] {
        let chars = Array(hex)
        guard chars.count % 2 == 0 else {
            throw Img4Error.operationFailed("invalid hex string length")
        }
        var bytes: [UInt8] = []
        for i in stride(from: 0, to: chars.count, by: 2) {
            guard let byte = UInt8(String(chars[i ... i + 1]), radix: 16) else {
                throw Img4Error.operationFailed("invalid hex character")
            }
            bytes.append(byte)
        }
        return bytes
    }
}

// MARK: - Signature Verification

enum Img4SignatureVerifier {
    /// Verify an IM4M signature using Security.framework.
    /// Returns true if the signature is valid, false otherwise.
    static func verifyIM4MSignature(_ im4mData: Data) -> Bool {
        guard let element = try? DERElement.parse(im4mData),
              element.tag == DERTag.sequence
        else {
            return false
        }

        guard let children = try? element.children(),
              children.count >= 5
        else {
            return false
        }

        // children[0] = IA5String "IM4M"
        // children[1] = INTEGER version
        // children[2] = SET (manifest body) - this is the signed data
        // children[3] = OCTET STRING (signature)
        // children[4] = SEQUENCE (certificate chain)

        let signedDataElement = children[2]
        let signatureElement = children[3]
        let certChainElement = children[4]

        guard signatureElement.tag == DERTag.octetString else { return false }

        let signedData = signedDataElement.raw
        let signature = signatureElement.value

        // Extract the first certificate from the chain
        guard let certChildren = try? certChainElement.children(),
              let firstCertElement = certChildren.first
        else {
            return false
        }

        let certData = firstCertElement.raw

        let valid = certData.withUnsafeBytes { cert in
            signedData.withUnsafeBytes { body in
                signature.withUnsafeBytes { sig in
                    img4_rsa_verify(cert.bindMemory(to: UInt8.self).baseAddress, cert.count,
                                    body.bindMemory(to: UInt8.self).baseAddress, body.count,
                                    sig.bindMemory(to: UInt8.self).baseAddress, sig.count)
                }
            }
        }
        if valid == 1 { return true }
        return false
    }
}
