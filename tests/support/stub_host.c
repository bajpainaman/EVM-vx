/* Host callbacks for unit-test builds of libevmvx.so. Every call reports a host error. */
#include <stdint.h>
uint64_t evmvx_host_account(void *h, void *a, void *o) { (void)h; (void)a; (void)o; return 2; }
uint64_t evmvx_host_storage(void *h, void *a, void *k, void *o) { (void)h; (void)a; (void)k; (void)o; return 2; }
uint64_t evmvx_host_block_hash(void *h, uint64_t n, void *o) { (void)h; (void)n; (void)o; return 2; }
uint64_t evmvx_crypto_precompile(uint64_t id, uint64_t spec, void *in, uint64_t len, uint64_t gas, void *out, uint64_t cap, uint64_t *res) { (void)id; (void)spec; (void)in; (void)len; (void)gas; (void)out; (void)cap; res[0] = 0; res[1] = 0; return 2; }
