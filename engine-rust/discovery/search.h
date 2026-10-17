#ifndef WEBTOR_SEARCH_H
#define WEBTOR_SEARCH_H
#include <stdint.h>
void *webtor_search_new(void);
void webtor_search_free(void *handle);
int webtor_search_add(void *handle, const uint8_t *data, uintptr_t length);
int webtor_search_query(void *handle, const char *query, uint8_t *out, uintptr_t capacity);
int webtor_dump_lookup_many(const uint8_t *hashes, uintptr_t count, WebtorDumpIter cb, void *ctx);
#endif
