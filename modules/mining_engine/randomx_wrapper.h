// modules/mining_engine/randomx_wrapper.h
#ifndef RANDOMX_WRAPPER_H
#define RANDOMX_WRAPPER_H

#include <stddef.h> // For size_t
#include <stdint.h> // For uint32_t, uint64_t

// Opaque structs
typedef struct randomx_cache randomx_cache;
typedef struct randomx_dataset randomx_dataset;
typedef struct randomx_vm randomx_vm;

// Flags (matching placeholders in lib.rs)
static const int RANDOMX_FLAG_DEFAULT = 0;
static const int RANDOMX_FLAG_LARGE_PAGES = 1;
// Add other flags like JIT, HARD_AES, FULL_MEM as needed by your design
// static const int RANDOMX_FLAG_FULL_MEM = 2;
// static const int RANDOMX_FLAG_JIT = 4;
// static const int RANDOMX_FLAG_HARD_AES = 8; // If you plan to support it

// Function declarations
randomx_cache* randomx_alloc_cache(uint32_t flags);
void randomx_init_cache(randomx_cache* cache, const void* key, size_t key_size);
void randomx_release_cache(randomx_cache* cache);

randomx_vm* randomx_create_vm(uint32_t flags, randomx_cache* cache, randomx_dataset* dataset);
void randomx_destroy_vm(randomx_vm* vm);
void randomx_vm_set_cache(randomx_vm* vm, randomx_cache* cache);

void randomx_calculate_hash(randomx_vm* vm, const void* input, size_t input_size, void* output);

// Optional: if you want to support iterative hashing for the same input block with different nonces
// void randomx_calculate_hash_first(randomx_vm* vm, const void* input, size_t input_size);
// void randomx_calculate_hash_next(randomx_vm* vm, const void* input, size_t input_size, void* output);

// Optional: if you plan to support full dataset initialization from Rust side
// randomx_dataset* randomx_alloc_dataset(uint32_t flags);
// uint64_t randomx_dataset_item_count();
// void randomx_init_dataset(randomx_dataset* dataset, randomx_cache* cache, uint64_t start_item, uint64_t item_count);
// void randomx_release_dataset(randomx_dataset* dataset);

#endif // RANDOMX_WRAPPER_H
