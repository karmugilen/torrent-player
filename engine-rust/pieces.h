#include <stdint.h>
#include <stddef.h>

typedef struct { uint32_t length, flags; } WebtorPieceRun;
typedef struct { uint32_t start, end; } WebtorPieceRange;
typedef struct {
    uint32_t start, end, total, selected, verified, receiving;
    uint32_t selected_verified, selected_receiving;
} WebtorPieceBucket;

int32_t webtor_piece_buckets(const WebtorPieceRun *, size_t,
    WebtorPieceRange *, size_t, uint32_t, uint32_t, WebtorPieceBucket *, size_t);

/* fd + [file_offset, file_offset+length) concatenated in piece order. */
typedef struct {
    int32_t fd;
    int32_t _pad;
    int64_t file_offset;
    int64_t length;
} WebtorFileSpan;

/* SHA-1 of logical bytes, hashing holes as zeros without reading them when
   SEEK_DATA works. Returns 0 (hashed), 1 (all holes; hash still written), or -1. */
int32_t webtor_hash_extents(const WebtorFileSpan *spans, size_t count, uint8_t out_hash[20]);
/* Concatenated pread into buf. Returns bytes copied, or -1. */
int32_t webtor_pread_extents(const WebtorFileSpan *spans, size_t count, uint8_t *buf, size_t len);
