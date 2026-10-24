# Img4tool (Linux)

[libimg4-spm](https://github.com/Lakr233/libimg4-spm) 0.1.1 (`1ed44d4`), the version vphone-cli
2.12.1 pins, by Lakr233 under the MIT license (`LICENSE`), adapted to build on Linux:

- `Crypto.swift`: AES-256-CBC and the RSA signature check use OpenSSL (`Shims/Img4OpenSSL`) instead of
  CommonCrypto and Security.framework; same functions, inputs and results.
- `Decompression.swift`: imports `VLCompression` (the Linux stand-in for Apple's Compression) instead
  of `Compression`.

`DER.swift` and `Img4tool.swift` are unchanged.
