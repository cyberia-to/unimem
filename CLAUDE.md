# Claude Code Instructions

## project: cyb-mem

pure Rust memory driver for Apple Silicon. IOSurface-backed pinned shared
buffers, bump arena (~1ns alloc), fixed-size tensor pool. zero-copy sharing
between CPU, GPU, AMX, and ANE.

## role in the stack

cyb-mem is a hardware memory driver. it allocates and manages buffers.
it does NOT run compute, compile shaders, build graphs, or schedule ops.

```
cyb-mem      memory: IOSurface, arena, pool
acpu         driver: CPU/AMX compute (NEON, AMX inline asm)
aruminium    driver: Metal GPU compute (shaders, pipelines)
rane         driver: ANE hardware (MIL compile, dispatch)
  ↑ drivers — raw hardware access, no model knowledge
──────────────────────────────────────────────────────
  ↓ runtimes — model graphs, scheduling, inference logic
cyb/llm      runtime: graph IR, jets, scheduling, model loading
```

all inference logic (attention blocks, transformer layers, model loading,
op scheduling, graph optimization) belongs in the runtime layer
(https://github.com/cyberia-to/cyb), not in the drivers.

drivers expose raw capabilities. runtimes compose them.

## architecture

```
src/
  lib.rs          public API: Surface, Arena, Pool, MemError
  ffi.rs          IOSurface + CoreFoundation raw FFI
  surface.rs      Surface: pinned IOSurface, locked at creation
  arena.rs        Arena: atomic bump allocator over Surface
  pool.rs         Pool: fixed-size tensor slots over Arena
  multi.rs        InferenceMemory: weights/activations/kv_cache layout
```

## source of truth

`specs/` is canonical. if specs/ and code disagree, resolve
in specs/ first, then propagate to code.

## build & verify

```bash
cargo build
cargo test
cargo bench
cargo run --example pipeline --release
```

no special signing. no entitlements. no SIP changes.

## key gotchas

- IOSurface locked once at creation, unlocked at drop. VA stable for lifetime.
- Arena alloc is compare_exchange loop, not fetch_add (no space waste on overshoot).
- Apple Silicon uses 16KB kernel pages, not 4KB.
- Surface is Send+Sync (immutable after creation). Arena is Send+Sync (atomic cursor).
- Pool Slot has lifetime tied to Pool — compile-time use-after-free prevention.
- IOSurfaceRef from surface.as_raw() is directly compatible with rane and aruminium.

## sibling drivers

- acpu (https://github.com/cyberia-to/acpu) — CPU/AMX: sgemm, softmax, rmsnorm, rope, silu
- aruminium (https://github.com/cyberia-to/aruminium) — Metal GPU: shaders, buffers, compute
- rane (https://github.com/cyberia-to/rane) — ANE: MIL compile, load, run

## coding conventions

- raw FFI to IOSurface.framework and CoreFoundation. no objc2, no wrapper crates.
- `cargo fmt` enforced. clippy clean.
- unsafe confined to ffi.rs and Surface/Arena internals.

## git workflow

- atomic commits — one logical change per commit
- conventional prefixes: feat:, fix:, refactor:, docs:, test:, chore:
- commit by default after completing a change

## license

cyber license: don't trust. don't fear. don't beg.
