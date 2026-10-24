#include "compression.h"
#include <lzfse.h>
#include <lzma.h>
#include <stdlib.h>
#include <string.h>
#include <zlib.h>

size_t compression_encode_buffer(uint8_t *dst, size_t dst_size, const uint8_t *src, size_t src_size,
                                 void *scratch, compression_algorithm algorithm)
{
    (void)scratch;
    switch (algorithm) {
    case COMPRESSION_LZFSE:
        return lzfse_encode_buffer(dst, dst_size, src, src_size, NULL);
    case COMPRESSION_ZLIB: {
        z_stream z = {0};
        if (deflateInit2(&z, Z_DEFAULT_COMPRESSION, Z_DEFLATED, -15, 8, Z_DEFAULT_STRATEGY) != Z_OK) {
            return 0;
        }
        z.next_in = (Bytef *)src; z.avail_in = src_size; z.next_out = dst; z.avail_out = dst_size;
        int r = deflate(&z, Z_FINISH);
        size_t n = (r == Z_STREAM_END) ? z.total_out : 0;
        deflateEnd(&z);
        return n;
    }
    case COMPRESSION_LZMA: {
        size_t pos = 0;
        if (lzma_easy_buffer_encode(6, LZMA_CHECK_CRC64, NULL, src, src_size, dst, &pos, dst_size) != LZMA_OK) {
            return 0;
        }
        return pos;
    }
    default:
        return 0;
    }
}

size_t compression_decode_buffer(uint8_t *dst, size_t dst_size, const uint8_t *src, size_t src_size,
                                 void *scratch, compression_algorithm algorithm)
{
    (void)scratch;
    switch (algorithm) {
    case COMPRESSION_LZFSE:
        return lzfse_decode_buffer(dst, dst_size, src, src_size, NULL);
    case COMPRESSION_ZLIB: {
        z_stream z = {0};
        if (inflateInit2(&z, -15) != Z_OK) {
            return 0;
        }
        z.next_in = (Bytef *)src; z.avail_in = src_size; z.next_out = dst; z.avail_out = dst_size;
        int r = inflate(&z, Z_FINISH);
        // Like Apple's: a full destination buffer returns what fit.
        size_t n = (r == Z_STREAM_END || r == Z_BUF_ERROR || r == Z_OK) ? z.total_out : 0;
        inflateEnd(&z);
        return n;
    }
    case COMPRESSION_LZMA: {
        lzma_stream s = LZMA_STREAM_INIT;
        if (lzma_stream_decoder(&s, UINT64_MAX, 0) != LZMA_OK) {
            return 0;
        }
        s.next_in = src; s.avail_in = src_size; s.next_out = dst; s.avail_out = dst_size;
        lzma_ret r = lzma_code(&s, LZMA_FINISH);
        size_t n = (r == LZMA_STREAM_END || r == LZMA_OK || r == LZMA_BUF_ERROR) ? s.total_out : 0;
        lzma_end(&s);
        return n;
    }
    default:
        return 0;
    }
}

typedef struct {
    compression_algorithm algorithm;
    z_stream z;
    lzma_stream x;
} stream_state;

compression_status compression_stream_init(compression_stream *stream,
                                           compression_stream_operation operation,
                                           compression_algorithm algorithm)
{
    if (operation != COMPRESSION_STREAM_DECODE) {
        return COMPRESSION_STATUS_ERROR; // vphone-cli only streams decodes
    }
    stream_state *st = calloc(1, sizeof(*st));
    if (!st) {
        return COMPRESSION_STATUS_ERROR;
    }
    st->algorithm = algorithm;
    if (algorithm == COMPRESSION_ZLIB) {
        if (inflateInit2(&st->z, -15) != Z_OK) { free(st); return COMPRESSION_STATUS_ERROR; }
    } else if (algorithm == COMPRESSION_LZMA) {
        lzma_stream init = LZMA_STREAM_INIT;
        st->x = init;
        if (lzma_stream_decoder(&st->x, UINT64_MAX, 0) != LZMA_OK) { free(st); return COMPRESSION_STATUS_ERROR; }
    } else {
        free(st);
        return COMPRESSION_STATUS_ERROR;
    }
    stream->state = st;
    return COMPRESSION_STATUS_OK;
}

compression_status compression_stream_process(compression_stream *stream, int flags)
{
    stream_state *st = stream->state;
    if (!st) {
        return COMPRESSION_STATUS_ERROR;
    }
    if (st->algorithm == COMPRESSION_ZLIB) {
        st->z.next_in = (Bytef *)stream->src_ptr; st->z.avail_in = stream->src_size;
        st->z.next_out = stream->dst_ptr; st->z.avail_out = stream->dst_size;
        int r = inflate(&st->z, (flags & COMPRESSION_STREAM_FINALIZE) ? Z_SYNC_FLUSH : Z_NO_FLUSH);
        stream->src_ptr = st->z.next_in; stream->src_size = st->z.avail_in;
        stream->dst_ptr = st->z.next_out; stream->dst_size = st->z.avail_out;
        if (r == Z_STREAM_END) return COMPRESSION_STATUS_END;
        return (r == Z_OK || r == Z_BUF_ERROR) ? COMPRESSION_STATUS_OK : COMPRESSION_STATUS_ERROR;
    }
    st->x.next_in = stream->src_ptr; st->x.avail_in = stream->src_size;
    st->x.next_out = stream->dst_ptr; st->x.avail_out = stream->dst_size;
    lzma_ret r = lzma_code(&st->x, (flags & COMPRESSION_STREAM_FINALIZE) ? LZMA_FINISH : LZMA_RUN);
    stream->src_ptr = st->x.next_in; stream->src_size = st->x.avail_in;
    stream->dst_ptr = st->x.next_out; stream->dst_size = st->x.avail_out;
    if (r == LZMA_STREAM_END) return COMPRESSION_STATUS_END;
    return (r == LZMA_OK || r == LZMA_BUF_ERROR) ? COMPRESSION_STATUS_OK : COMPRESSION_STATUS_ERROR;
}

compression_status compression_stream_destroy(compression_stream *stream)
{
    stream_state *st = stream->state;
    if (st) {
        if (st->algorithm == COMPRESSION_ZLIB) inflateEnd(&st->z);
        else lzma_end(&st->x);
        free(st);
    }
    stream->state = NULL;
    return COMPRESSION_STATUS_OK;
}
