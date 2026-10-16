#ifndef PARSEQUERADE_H
#define PARSEQUERADE_H
#include <stdbool.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
/* Returned UTF-8 JSON strings are owned by Rust. Free with pq_string_free.
 * Handles are opaque integer IDs, never pointers into a host language's heap.
 * Release operations in all paths; cancel first to stop outstanding work.
 * Client release prevents new operations but never invalidates active work. */
uint32_t pq_abi_version(void);
char *pq_client_new(const char *config);
void pq_client_free(uint64_t client);
char *pq_start(uint64_t client, const char *request);
char *pq_poll(uint64_t operation, uint32_t timeout_ms);
bool pq_respond(uint64_t operation, uint64_t event, const char *response);
void pq_cancel(uint64_t operation);
void pq_operation_free(uint64_t operation);
void pq_string_free(char *value);
#ifdef __cplusplus
}
#endif
#endif
