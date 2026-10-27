/* Atomic primitives for the Vx-side Block-STM scheduler (sched.vx).
 * Vx has no atomic intrinsics; these wrap C11 atomics behind plain
 * extern "C" symbols the Vx module declares. CPU builds only. */
#include <stdatomic.h>
#include <stdint.h>

uint64_t evx_aload(const uint64_t *p) {
    return atomic_load_explicit((const _Atomic uint64_t *)p, memory_order_acquire);
}
void evx_astore(uint64_t *p, uint64_t v) {
    atomic_store_explicit((_Atomic uint64_t *)p, v, memory_order_release);
}
uint64_t evx_afetch_add(uint64_t *p, uint64_t v) {
    return atomic_fetch_add_explicit((_Atomic uint64_t *)p, v, memory_order_acq_rel);
}
/* Compare-and-swap: returns 1 and swaps if *p == want, else 0. */
uint64_t evx_acas(uint64_t *p, uint64_t want, uint64_t new) {
    uint64_t expected = want;
    return atomic_compare_exchange_strong_explicit(
        (_Atomic uint64_t *)p, &expected, new,
        memory_order_acq_rel, memory_order_acquire) ? 1 : 0;
}
/* Exclusive/shared spinlock over one u64: bit 63 = writer, low bits = readers. */
void evx_lock_sh(uint64_t *p) {
    for (;;) {
        uint64_t v = evx_aload(p);
        if (!(v >> 63) && evx_acas(p, v, v + 1)) return;
    }
}
void evx_unlock_sh(uint64_t *p) { atomic_fetch_sub_explicit((_Atomic uint64_t *)p, 1, memory_order_acq_rel); }
void evx_lock_ex(uint64_t *p) {
    for (;;) {
        if (evx_acas(p, 0, (uint64_t)1 << 63)) return;
    }
}
void evx_unlock_ex(uint64_t *p) { evx_astore(p, 0); }
