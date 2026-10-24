// OpenSSL versions of the two primitives Img4tool takes from CommonCrypto and Security.framework.
#ifndef IMG4_OPENSSL_H
#define IMG4_OPENSSL_H
#include <stddef.h>
#include <stdint.h>

// AES-256-CBC with PKCS#7 padding. Returns 0 and the plaintext length in *out_len.
int img4_aes256_cbc_decrypt(const uint8_t *key, const uint8_t *iv, const uint8_t *in, size_t in_len,
                            uint8_t *out, size_t *out_len);

// RSA PKCS#1 v1.5 signature over `data` by the DER certificate's key, SHA-384 or SHA-1.
// Returns 1 when valid.
int img4_rsa_verify(const uint8_t *cert_der, size_t cert_len, const uint8_t *data, size_t data_len,
                    const uint8_t *sig, size_t sig_len);
#endif
