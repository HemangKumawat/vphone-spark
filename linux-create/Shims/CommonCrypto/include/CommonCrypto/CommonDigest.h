// Linux stand-in for Apple's <CommonCrypto/CommonDigest.h>.
//
// vphone-cli 2.12's openavb.c (the OpenAVB stage-0 iBSS description) calls
// CC_SHA256 twice and nothing else from CommonCrypto. OpenSSL already backs the
// image4 shim in this package, so map the two calls onto it instead of pulling
// in a CommonCrypto port.
#ifndef VPHONE_LINUX_COMMONCRYPTO_COMMONDIGEST_H
#define VPHONE_LINUX_COMMONCRYPTO_COMMONDIGEST_H

#include <openssl/sha.h>
#include <stddef.h>
#include <stdint.h>

typedef uint32_t CC_LONG;

#define CC_SHA256_DIGEST_LENGTH 32
#define CC_SHA256_DIGEST_LENGTH_SIZE 32

static inline unsigned char *CC_SHA256(const void *data, CC_LONG len, unsigned char *md)
{
    return SHA256((const unsigned char *)data, (size_t)len, md);
}

#endif // VPHONE_LINUX_COMMONCRYPTO_COMMONDIGEST_H
