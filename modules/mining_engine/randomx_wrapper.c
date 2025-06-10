// modules/mining_engine/randomx_wrapper.c
#include "randomx_wrapper.h"
#include <stdio.h>  // For printf (debugging)
#include <stdlib.h> // For malloc, free
#include <string.h> // For memcpy

// Dummy struct definitions (content doesn't matter for placeholder)
struct randomx_cache { int dummy_cache_data; };
struct randomx_dataset { int dummy_dataset_data; };
struct randomx_vm { int dummy_vm_data; };

randomx_cache* randomx_alloc_cache(uint32_t flags) {
    printf("[C WRAPPER] randomx_alloc_cache called with flags: %u\n", flags);
    randomx_cache* cache = (randomx_cache*)malloc(sizeof(randomx_cache));
    if (cache) cache->dummy_cache_data = 123;
    return cache;
}

void randomx_init_cache(randomx_cache* cache, const void* key, size_t key_size) {
    printf("[C WRAPPER] randomx_init_cache called with cache: %p, key_size: %zu\n", (void*)cache, key_size);
    // Do nothing with the key for this dummy version
    (void)key;
    if (cache) { /* ensure cache is not null */ }
}

void randomx_release_cache(randomx_cache* cache) {
    printf("[C WRAPPER] randomx_release_cache called with cache: %p\n", (void*)cache);
    free(cache);
}

randomx_vm* randomx_create_vm(uint32_t flags, randomx_cache* cache, randomx_dataset* dataset) {
    printf("[C WRAPPER] randomx_create_vm called with flags: %u, cache: %p, dataset: %p\n", flags, (void*)cache, (void*)dataset);
    (void)dataset; // Not using dataset in light VM mode for this dummy
    if (!cache) return NULL; // VM needs a cache
    randomx_vm* vm = (randomx_vm*)malloc(sizeof(randomx_vm));
    if (vm) vm->dummy_vm_data = 456;
    return vm;
}

void randomx_destroy_vm(randomx_vm* vm) {
    printf("[C WRAPPER] randomx_destroy_vm called with vm: %p\n", (void*)vm);
    free(vm);
}

void randomx_vm_set_cache(randomx_vm* vm, randomx_cache* cache) {
    printf("[C WRAPPER] randomx_vm_set_cache called with vm: %p, cache: %p\n", (void*)vm, (void*)cache);
    // Do nothing for this dummy version
     if (vm && cache) { /* ensure params are not null */ }
}

void randomx_calculate_hash(randomx_vm* vm, const void* input, size_t input_size, void* output) {
    printf("[C WRAPPER] randomx_calculate_hash called with vm: %p, input_size: %zu\n", (void*)vm, input_size);
    // Fill output with a dummy pattern (e.g., all zeros or a fixed pattern)
    // RandomX hash is 32 bytes.
    if (output) {
        memset(output, 0, 32); // Zero out the hash
        // Or a pattern: for(int i=0; i<32; ++i) ((char*)output)[i] = (char)i;
    }
    if (vm && input) { /* ensure params are not null */ }
}
