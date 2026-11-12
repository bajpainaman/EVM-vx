/* The engine's allocator (vx_malloc / vx_calloc / vx_realloc / vx_free in
 * src/evm/libc.vx call these). A size-class allocator with one heap per
 * thread, in the style of mimalloc:
 *
 * - Each heap keeps free lists per class and carves new blocks from large
 *   chunks that are mapped and pre-faulted in one call. Allocation and local
 *   frees take no lock and make no system call in the steady state (glibc's
 *   thread heaps grow with an mprotect every few pages, which serializes all
 *   cores on the kernel's memory-map lock).
 * - Every block's 16-byte header holds its class and its owning heap. A block
 *   freed by another thread goes onto the owner's lock-free remote list, and
 *   the owner drains that list when a free list runs dry. So memory returns
 *   to the thread that allocates it (workers allocate results that the main
 *   thread frees), and long multi-block runs stay bounded.
 * - Heaps are never freed. A permanent registry has immutable links; exiting
 *   threads release ownership and new threads claim an idle heap. Registry
 *   nodes are never removed, so adoption has no ABA/reclamation problem.
 *
 * Blocks above the biggest class go to libc, marked LARGE in the header.
 * CPU builds only; a device build replaces this file. */
#define _GNU_SOURCE
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>

#define NCLASS 13                   /* 16 B .. 64 KiB, powers of two */
#define MAX_CLASS_SIZE (16u << 12)
#define CHUNK (8u << 20)            /* 8 MiB per refill */
#define LARGE 0xffu
#define HDR 16u

typedef struct FreeBlock { struct FreeBlock *next; } FreeBlock;

typedef struct Heap {
    FreeBlock *lists[NCLASS];
    uint8_t *cur, *end;
    _Atomic(FreeBlock *) remote;     /* blocks freed by other threads */
    struct Heap *next;
    _Atomic unsigned claimed;
} Heap;

static _Atomic(Heap *) heaps;
static pthread_key_t exit_key;
static __thread Heap *my_heap;

static void on_thread_exit(void *h) {
    Heap *heap = (Heap *)h;
    atomic_store_explicit(&heap->claimed, 0, memory_order_release);
}

/* Library initialization precedes exported calls; no lazy-init spinlock. */
__attribute__((constructor)) static void make_key(void) {
    if (pthread_key_create(&exit_key, on_thread_exit) != 0) abort();
}

/* First use on a thread: adopt an orphaned heap, or make a new one. */
static Heap *heap_init(void) {
    Heap *h = atomic_load_explicit(&heaps, memory_order_acquire);
    while (h) {
        unsigned available = 0;
        if (atomic_compare_exchange_strong_explicit(&h->claimed, &available, 1,
                                                    memory_order_acquire, memory_order_relaxed)) break;
        h = h->next;
    }
    if (!h) {
        h = (Heap *)calloc(1, sizeof(Heap));
        if (!h) abort();
        atomic_init(&h->claimed, 1);
        atomic_init(&h->remote, NULL);
        Heap *head = atomic_load_explicit(&heaps, memory_order_acquire);
        do { h->next = head; }
        while (!atomic_compare_exchange_weak_explicit(&heaps, &head, h,
                                                      memory_order_acq_rel, memory_order_acquire));
    }
    my_heap = h;
    if (pthread_setspecific(exit_key, h) != 0) abort();
    return h;
}

static inline Heap *heap(void) { return my_heap ? my_heap : heap_init(); }

static inline unsigned class_of(uint64_t n) {
    unsigned c = 0;
    uint64_t s = 16;
    while (s < n) { s <<= 1; c++; }
    return c;
}

static inline uint64_t class_size(unsigned c) { return (uint64_t)16 << c; }

/* Moves every remotely freed block onto this heap's own lists. A remote
 * block keeps its class in header word 0 and its list link in word 1. */
static void drain_remote(Heap *h) {
    uint64_t *b = (uint64_t *)atomic_exchange_explicit(&h->remote, NULL, memory_order_acquire);
    while (b) {
        uint64_t *next = (uint64_t *)(uintptr_t)b[1];
        unsigned c = (unsigned)b[0];
        FreeBlock *fb = (FreeBlock *)b;
        fb->next = h->lists[c];
        h->lists[c] = fb;
        b = next;
    }
}

static uint8_t *carve(Heap *h, uint64_t total) {
    if (h->cur == NULL || (uint64_t)(h->end - h->cur) < total) {
        void *m = mmap(NULL, CHUNK, PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANONYMOUS | MAP_POPULATE, -1, 0);
        if (m == MAP_FAILED) return NULL;
        h->cur = (uint8_t *)m;
        h->end = h->cur + CHUNK;
    }
    uint8_t *p = h->cur;
    h->cur += total;
    return p;
}

void *evx_malloc(uint64_t n) {
    if (n == 0) n = 1;
    if (n > MAX_CLASS_SIZE) {
        if (n > UINT64_MAX - HDR) return NULL;
        uint8_t *p = (uint8_t *)malloc(n + HDR);
        if (!p) return NULL;
        ((uint64_t *)p)[0] = LARGE;
        ((uint64_t *)p)[1] = n;
        return p + HDR;
    }
    Heap *h = heap();
    unsigned c = class_of(n);
    if (!h->lists[c] && atomic_load_explicit(&h->remote, memory_order_relaxed)) drain_remote(h);
    FreeBlock *b = h->lists[c];
    uint8_t *p;
    if (b) {
        h->lists[c] = b->next;
        p = (uint8_t *)b;
    } else {
        p = carve(h, class_size(c) + HDR);
        if (!p) return NULL;
    }
    ((uint64_t *)p)[0] = c;
    ((uint64_t *)p)[1] = (uint64_t)(uintptr_t)h;
    return p + HDR;
}

void *evx_calloc(uint64_t n, uint64_t size) {
    uint64_t total;
    if (__builtin_mul_overflow(n, size, &total)) return NULL;
    if (total > MAX_CLASS_SIZE) {
        if (total > UINT64_MAX - HDR) return NULL;
        /* libc calloc maps fresh zero pages for big blocks and never touches
         * them; malloc + memset would fault in every page up front. */
        uint8_t *p = (uint8_t *)calloc(1, total + HDR);
        if (!p) return NULL;
        ((uint64_t *)p)[0] = LARGE;
        ((uint64_t *)p)[1] = total;
        return p + HDR;
    }
    void *p = evx_malloc(total);
    if (p) memset(p, 0, total == 0 ? 1 : total);
    return p;
}

void evx_free(void *ptr) {
    if (!ptr) return;
    uint8_t *p = (uint8_t *)ptr - HDR;
    uint64_t c = ((uint64_t *)p)[0];
    if (c == LARGE) { free(p); return; }
    Heap *owner = (Heap *)(uintptr_t)((uint64_t *)p)[1];
    if (owner == my_heap) {
        /* Local free: the list link overwrites header word 0. */
        FreeBlock *b = (FreeBlock *)p;
        b->next = owner->lists[c];
        owner->lists[c] = b;
        return;
    }
    /* Remote free: word 0 keeps the class for the owner's drain, and the
     * link goes in word 1 (the owner pointer, no longer needed). */
    FreeBlock *head = atomic_load_explicit(&owner->remote, memory_order_relaxed);
    do {
        ((uint64_t *)p)[1] = (uint64_t)(uintptr_t)head;
    } while (!atomic_compare_exchange_weak_explicit(&owner->remote, &head, (FreeBlock *)p,
                                                    memory_order_release, memory_order_relaxed));
}

void *evx_realloc(void *ptr, uint64_t n) {
    if (!ptr) return evx_malloc(n);
    uint8_t *p = (uint8_t *)ptr - HDR;
    uint64_t c = ((uint64_t *)p)[0];
    uint64_t have = (c == LARGE) ? ((uint64_t *)p)[1] : class_size((unsigned)c);
    if (c != LARGE && n <= have) return ptr;
    void *q = evx_malloc(n);
    if (!q) return NULL;
    memcpy(q, ptr, have < n ? have : n);
    evx_free(ptr);
    return q;
}
