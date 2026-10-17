#ifndef WEBTOR_DISCOVERY_H
#define WEBTOR_DISCOVERY_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct WebtorDumpRow {
	char name[512];
	int64_t size;
	int32_t seeders;
	int32_t leechers;
} WebtorDumpRow;

/* Load mmap index. Returns row count or -1. */
int webtor_dump_load(const char *path);
void webtor_dump_free(void);
int webtor_dump_len(void);
/* 1 hit, 0 miss, -1 error. hash is 40-char hex. */
int webtor_dump_lookup(const char *hash, WebtorDumpRow *out);
/* Callback: return 0 to stop. Returns rows visited. */
typedef int (*WebtorDumpIter)(const char *hash, const WebtorDumpRow *row, void *ctx);
int webtor_dump_for_each_seeded(WebtorDumpIter cb, void *ctx);
/* Download if missing/stale. force!=0 always. 0 ok, -1 fail. */
int webtor_dump_ensure(const char *path, int force);

/* Catalog JSON array into *json_out (caller frees with webtor_string_free). */
int webtor_catalog_fetch(const char *query, int size, int after, char **json_out, int *next_out);
void webtor_string_free(char *s);

#ifdef __cplusplus
}
#endif

#endif /* WEBTOR_DISCOVERY_H */
