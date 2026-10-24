// Linux stand-in for Apple's libcompression: the subset vphone-cli uses, with Apple's names,
// values and struct layout. LZFSE via liblzfse, ZLIB (raw DEFLATE, as Apple's is) via zlib,
// LZMA (the xz container, as Apple's is) via liblzma.
#ifndef VPHONE_LINUX_COMPRESSION_H
#define VPHONE_LINUX_COMPRESSION_H
#include <stddef.h>
#include <stdint.h>

typedef enum {
    COMPRESSION_LZ4 = 0x100,
    COMPRESSION_ZLIB = 0x205,
    COMPRESSION_LZMA = 0x306,
    COMPRESSION_LZ4_RAW = 0x101,
    COMPRESSION_LZFSE = 0x801,
} compression_algorithm;

typedef enum {
    COMPRESSION_STATUS_OK = 0,
    COMPRESSION_STATUS_ERROR = -1,
    COMPRESSION_STATUS_END = 1,
} compression_status;

typedef enum {
    COMPRESSION_STREAM_ENCODE = 0,
    COMPRESSION_STREAM_DECODE = 1,
} compression_stream_operation;

typedef enum {
    COMPRESSION_STREAM_FINALIZE = 0x0001,
} compression_stream_flags;

typedef struct {
    uint8_t *dst_ptr;
    size_t dst_size;
    const uint8_t *src_ptr;
    size_t src_size;
    void *state;
} compression_stream;

size_t compression_encode_buffer(uint8_t *dst, size_t dst_size, const uint8_t *src, size_t src_size,
                                 void *scratch, compression_algorithm algorithm);
size_t compression_decode_buffer(uint8_t *dst, size_t dst_size, const uint8_t *src, size_t src_size,
                                 void *scratch, compression_algorithm algorithm);
compression_status compression_stream_init(compression_stream *stream,
                                           compression_stream_operation operation,
                                           compression_algorithm algorithm);
compression_status compression_stream_process(compression_stream *stream, int flags);
compression_status compression_stream_destroy(compression_stream *stream);
#endif
