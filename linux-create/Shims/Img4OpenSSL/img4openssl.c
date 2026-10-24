#include "img4openssl.h"
#include <openssl/evp.h>
#include <openssl/x509.h>

int img4_aes256_cbc_decrypt(const uint8_t *key, const uint8_t *iv, const uint8_t *in, size_t in_len,
                            uint8_t *out, size_t *out_len)
{
    EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
    int n = 0, f = 0, ok = 0;
    if (ctx && EVP_DecryptInit_ex(ctx, EVP_aes_256_cbc(), NULL, key, iv) == 1 &&
        EVP_DecryptUpdate(ctx, out, &n, in, (int)in_len) == 1 &&
        EVP_DecryptFinal_ex(ctx, out + n, &f) == 1) {
        *out_len = (size_t)n + (size_t)f;
        ok = 1;
    }
    EVP_CIPHER_CTX_free(ctx);
    return ok ? 0 : -1;
}

static int verify_with(EVP_PKEY *key, const EVP_MD *md, const uint8_t *data, size_t data_len,
                       const uint8_t *sig, size_t sig_len)
{
    EVP_MD_CTX *ctx = EVP_MD_CTX_new();
    int ok = ctx && EVP_DigestVerifyInit(ctx, NULL, md, NULL, key) == 1 &&
             EVP_DigestVerify(ctx, sig, sig_len, data, data_len) == 1;
    EVP_MD_CTX_free(ctx);
    return ok;
}

int img4_rsa_verify(const uint8_t *cert_der, size_t cert_len, const uint8_t *data, size_t data_len,
                    const uint8_t *sig, size_t sig_len)
{
    const unsigned char *p = cert_der;
    X509 *cert = d2i_X509(NULL, &p, (long)cert_len);
    if (!cert) {
        return 0;
    }
    EVP_PKEY *key = X509_get0_pubkey(cert);
    int ok = key && (verify_with(key, EVP_sha384(), data, data_len, sig, sig_len) ||
                     verify_with(key, EVP_sha1(), data, data_len, sig, sig_len));
    X509_free(cert);
    return ok;
}
