# cyb-mem Specification Audit

Audit date: 2026-03-31

---

## CRITICAL: Layer 1 (phys.rs) cannot work as specified

### IOMallocContiguous is kernel-only

The spec declares direct FFI calls to `IOMallocContiguous`, `IOFreeContiguous`, and `IOMemoryDescriptorGetPhysicalAddress`. None of these are callable from userspace.

Verified against MacOSX SDK headers and IOKit.tbd symbol table:
- `IOMallocContiguous` — kernel-only (kext API). Not exported in IOKit.framework userspace headers. Not in IOKit.tbd.
- `IOFreeContiguous` — kernel-only. Same.
- `IOMemoryDescriptorGetPhysicalAddress` — not a C function at all. It's a C++ method on the `IOMemoryDescriptor` kernel class.

The spec says "same pattern as aruminium" — but aruminium uses Metal (ObjC runtime FFI), not IOKit physical allocation. aruminium never touches physical addresses.

The spec says "same pattern as rane" — but rane uses IOSurface (kernel-managed shared memory with virtual addressing). rane never touches physical addresses either.

Neither reference project validates the claimed approach.

### Consequence

The entire crate is built on Layer 1. If phys.rs can't allocate physically contiguous memory and return physical addresses from userspace, every downstream layer is affected:
- arena.rs PA arithmetic won't work (no base_pa to offset from)
- pool.rs Slot.pa won't work (no PA to cache)
- dma.rs DmaTarget.submit(pa, ...) won't work (no PA to submit)
- The zero-copy pipeline breaks — hardware can't be addressed by PA

---

## Layer 2 (hyp.rs): Hypervisor API is real but misunderstood

### Functions are verified

All four Hypervisor.framework functions exist and are callable from userspace (confirmed in hv_vm.h, Hypervisor.tbd, API_AVAILABLE macOS 11.0+):
- `hv_vm_create(config)` → `hv_return_t`
- `hv_vm_map(addr, ipa, size, flags)` → `hv_return_t`
- `hv_vm_unmap(ipa, size)` → `hv_return_t`
- `hv_vm_protect(ipa, size, flags)` → `hv_return_t`

### Signatures differ from spec

Spec declares:
```
hv_vm_map(uva: *mut u8, ipa: u64, size: usize, flags: u64)
```

Actual signature:
```c
hv_return_t hv_vm_map(void *addr, hv_ipa_t ipa, size_t size, hv_memory_flags_t flags);
```

`hv_ipa_t` is `uint64_t`, `hv_memory_flags_t` is `uint64_t` — types match but should use proper type aliases.

### IPA is NOT physical address

The spec implies that Hypervisor mapping gives "deterministic latency" and maps "physical pages into guest IPA space." This conflates two things:

- IPA (Intermediate Physical Address) is a guest-physical address visible only to guest VCPUs
- IPA is NOT a real physical address — it goes through stage-2 translation
- `hv_vm_map` maps a HOST virtual address to a GUEST physical address
- Without running code inside a guest VCPU, the IPA is useless for AMX/ANE/NVMe

The Hypervisor path only helps if we run inference code inside a guest VCPU — which brings its own overhead (VCPU entry/exit ~1us per transition). The spec doesn't mention this.

### TLB isolation claim is partially true

Stage-2 TLB entries are separate from stage-1 — true. But the mapped memory still goes through stage-1 translation on the host side. The "deterministic latency" claim only holds for code running inside the VM, which has its own performance costs.

---

## What DOES work from userspace on Apple Silicon

### IOSurface (proven by rane)

IOSurface provides:
- Pinned, wired memory that is never swapped
- Accessible to CPU and hardware (GPU, ANE) through DART/IOMMU
- Zero-copy sharing between CPU and hardware units
- No physical address exposure — hardware accesses through DART mappings

rane's approach: `IOSurfaceCreate` with size properties → `IOSurfaceLock/Unlock` for CPU access → pass IOSurfaceRef to ANE via private ObjC API. Works today for ANE inference.

Limitation: no physical address. Hardware addressing goes through DART.

### IOKit User Client (IOConnectMapMemory64)

Userspace-available IOKit functions for memory:
- `IOServiceGetMatchingService` — find a driver
- `IOServiceOpen` — open connection
- `IOConnectCallStructMethod` — invoke driver methods (including allocation)
- `IOConnectMapMemory64` — map driver-allocated memory into userspace

This requires a driver on the kernel side that supports contiguous allocation. Potential candidates:
- GPU driver (IOGPU/AGXAccelerator) user client — allocates contiguous GPU buffers
- IOAccelerator framework — wraps GPU memory allocation

Limitation: depends on private driver interfaces that Apple can change.

### Mach VM + mlock

- `mach_vm_allocate` — allocate virtual memory with large page hints
- `mlock` / `mach_vm_wire` — pin pages (prevent swap)
- Pages stay resident but: no contiguity guarantee, no PA visibility

### DriverKit Extension (DEXT)

A userspace driver extension CAN:
- Allocate `IOBufferMemoryDescriptor` with `kIOMemoryPhysicallyContiguous`
- Map to user client via `IOConnectMapMemory64`
- Retrieve physical addresses within the DEXT process

This is the "correct" Apple-sanctioned path for contiguous physical memory. Requires writing and signing a DEXT.

---

## Spec issues beyond Layer 1

### arena.rs: PA arithmetic assumes contiguity

The formula `pa = base_pa + (ptr - base_va)` requires physically contiguous backing. Even if we solve Layer 1, large contiguous allocations (1 GB as in the pipeline example) may not be possible — the kernel may not have 1 GB of contiguous physical memory available.

Realistic maximum for contiguous allocation: 16-64 MB depending on system memory pressure. For 1 GB, we'd need a scatter-gather approach with per-page PA tracking.

### arena.rs: alloc race condition

The spec says alloc uses `fetch_add` but the api-sketch shows `compare_exchange_weak`. The actual implementation needs care:

With `fetch_add`: if two threads allocate simultaneously and the second one exceeds capacity, the cursor is already advanced past capacity. Need to handle rollback or accept wasted space.

With `compare_exchange`: correct but slower under contention (retry loop).

The spec should specify which approach and document the tradeoff.

### arena.rs: concurrent alloc + reset

Spec notes "concurrent alloc + reset is NOT safe" but doesn't specify who is responsible. If the arena is used by multiple threads (which atomic alloc suggests), reset requires external synchronization. The spec should define the synchronization protocol.

### pool.rs: Slot lifetime

Slot holds a raw pointer. If the pool's arena is dropped while slots are outstanding, the pointer dangles. The spec doesn't define the lifetime relationship between Pool and Slot.

Options:
- Slot borrows Pool (Rust lifetime prevents use-after-free)
- Pool is reference-counted (Slot holds Arc)
- User's responsibility (unsafe contract)

The spec should pick one.

### pool.rs: SLOT_SIZE as const generic

`as_mut_slice(&mut self, len: usize)` takes a runtime len but SLOT_SIZE is compile-time. The assertion `len <= SLOT_SIZE` can't be checked at compile time. This is fine but should be documented as a runtime check.

### dma.rs: DmaTarget is a trait without implementors

cyb-mem defines the trait but no hardware crate implements it yet. The trait design can't be validated without at least one real implementation. Risk: the trait may need redesign once AMX/ANE/NVMe crates try to implement it.

Specific concern: `submit` takes `(pa, size, op)` — this may be too simple. ANE needs compiled programs + multiple input/output buffers. AMX needs operation type + matrix dimensions. NVMe needs LBA + queue info.

### dma.rs: DmaToken ownership

`poll` takes `&DmaToken`, `wait` takes `DmaToken` (by value, consuming it). But what if you want to poll, find it's not done, and then wait? After `poll` you still have the token. After `wait` it's consumed. This is fine but the spec should document: poll is non-destructive, wait is terminal.

### map.rs: resolve_range returns Vec

`resolve_range` returns `Vec<(u64, usize)>` — this allocates. In a crate that targets < 5ns allocations, a Vec allocation in the hot path is inconsistent. Should accept a pre-allocated buffer or use a fixed-size array.

Also: `resolve_range` is the only function that prevents `no_std` compatibility (Vec requires alloc). Spec says "no_std compatible" for arena but doesn't clarify for map.

### Error model: HvCreateFailed(i32) carries error code

Good — but `AllocFailed` doesn't carry any diagnostic info. On IOKit failure, the kern_return_t code is useful for debugging. Consider `AllocFailed(kern_return_t)`.

### Benchmark targets may be unrealistic

- "Sequential read 1GB > 70 GB/s" — M1 Max memory bandwidth is 400 GB/s shared across all units. 70 GB/s from a single core is plausible with NEON/AMX but needs careful measurement.
- "Random 4K read < 50ns" — with TLB hit this is ~30ns, with TLB miss it's 100ns+. The 50ns target assumes TLB hit rate, which is what Hypervisor stage-2 is supposed to help with.
- "847 tok/s vs 201 tok/s" — 4.2x improvement from zero-copy alone is aggressive. Memory copies are ~15% of bandwidth; the remaining bottleneck is compute. Realistic improvement from zero-copy: 1.3-1.5x. The 4.2x claim implies other optimizations (AMX/ANE direct access) beyond what cyb-mem alone provides.

---

## Recommended path forward

### Option A: IOSurface-based (least risk, proven)

Replace Layer 1 with IOSurface. Accept no physical address visibility. Hardware access goes through DART — which is exactly how rane already works for ANE.

Pros: works today, proven, no kernel component needed
Cons: no PA, can't bypass DART, DMA trait needs redesign

### Option B: DEXT-based (correct, more work)

Write a DriverKit extension that allocates contiguous physical memory and exposes it to userspace.

Pros: real PAs, Apple-sanctioned, survives OS updates
Cons: requires DEXT signing, more complex build, Apple review for distribution

### Option C: IOKit User Client to existing driver (hacky, fast)

Find an existing system driver that exposes contiguous allocation through its user client interface. Use IOConnectCallStructMethod to trigger kernel-side IOMallocContiguous indirectly.

Pros: no kernel component to write, real PAs possible
Cons: depends on private interfaces, may break on OS update

### Option D: Mach VM + Hypervisor hybrid (pragmatic)

Use mach_vm_allocate for large allocations, mlock to pin, Hypervisor.framework for deterministic addressing within a guest. Accept that "physical address" means "guest IPA" and run compute in VCPU context.

Pros: all public APIs, no kernel component
Cons: VCPU overhead, IPA is not real PA, complex execution model

### Recommendation

Start with Option A (IOSurface) to validate the arena/pool/DMA architecture. This is what rane already proves works. Then pursue Option B (DEXT) for the real physical address path as a separate effort.

The spec should be split:
- v0: IOSurface-backed, virtual addresses, DART-mediated hardware access
- v1: DEXT-backed, physical addresses, direct hardware DMA

---

## Summary of issues

| # | Severity | Layer | Issue |
|---|----------|-------|-------|
| 1 | CRITICAL | phys | IOMallocContiguous is kernel-only, not callable from userspace |
| 2 | CRITICAL | phys | IOFreeContiguous is kernel-only |
| 3 | CRITICAL | phys | IOMemoryDescriptorGetPhysicalAddress is not a C function |
| 4 | HIGH | hyp | IPA is guest-physical, not real physical — needs VCPU to be useful |
| 5 | HIGH | arena | 1 GB contiguous physical allocation is unrealistic |
| 6 | HIGH | dma | DmaTarget trait too simple for real hardware (ANE needs programs, not just PA+size) |
| 7 | HIGH | all | "Same pattern as aruminium/rane" is false — neither uses physical addresses |
| 8 | MEDIUM | arena | fetch_add vs compare_exchange tradeoff unspecified |
| 9 | MEDIUM | arena | Concurrent alloc + reset synchronization protocol undefined |
| 10 | MEDIUM | pool | Slot lifetime vs Pool lifetime undefined |
| 11 | MEDIUM | map | resolve_range allocates Vec, breaks no_std and perf goals |
| 12 | MEDIUM | dma | DmaToken poll/wait ownership semantics should be explicit |
| 13 | LOW | error | AllocFailed should carry diagnostic code |
| 14 | LOW | bench | 4.2x inference speedup claim is aggressive for zero-copy alone |
| 15 | LOW | hyp | Function signatures use wrong type aliases |
