/* Host callbacks for unit-test builds of libevmvx.so. Every call reports a host error. */
#include <stdint.h>
uint64_t evmvx_host_account(void *h, void *a, void *o) { (void)h; (void)a; (void)o; return 2; }
uint64_t evmvx_host_storage(void *h, void *a, void *k, void *o) { (void)h; (void)a; (void)k; (void)o; return 2; }
uint64_t evmvx_host_block_hash(void *h, uint64_t n, void *o) { (void)h; (void)n; (void)o; return 2; }
