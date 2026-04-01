# cyb-mem: API Reference

Extracted from the actual implementation in `src/`.

---

## lib.rs

```rust
pub mod ffi;
pub mod surface;
pub mod arena;
pub mod pool;

pub use surface::Surface;
pub use arena::Arena;
pub use pool::{Pool, Slot};

#[derive(Debug)]
pub enum MemError {
    ZeroSize,
    SurfaceCreateFailed,
    SurfaceLockFailed(i32),
}
```

---

## ffi.rs

IOSurface + CoreFoundation raw FFI. No objc2, no wrappers.

```rust
// Types
pub type IOSurfaceRef = *mut c_void;
pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFMutableDictionaryRef = *mut c_void;
pub type kern_return_t = i32;

// IOSurface.framework
extern "C" {
    pub fn IOSurfaceCreate(properties: CFMutableDictionaryRef) -> IOSurfaceRef;
    pub fn IOSurfaceLock(surface: IOSurfaceRef, options: u32, seed: *mut u32) -> kern_return_t;
    pub fn IOSurfaceUnlock(surface: IOSurfaceRef, options: u32, seed: *mut u32) -> kern_return_t;
    pub fn IOSurfaceGetBaseAddress(surface: IOSurfaceRef) -> *mut c_void;
    pub fn IOSurfaceGetAllocSize(surface: IOSurfaceRef) -> usize;
    pub fn IOSurfaceGetID(surface: IOSurfaceRef) -> u32;
}

// CoreFoundation
extern "C" {
    pub fn CFDictionaryCreateMutable(...) -> CFMutableDictionaryRef;
    pub fn CFDictionarySetValue(dict, key, value);
    pub fn CFNumberCreate(allocator, theType, valuePtr) -> *const c_void;
    pub fn CFRelease(cf: CFTypeRef);
    pub static kCFTypeDictionaryKeyCallBacks: c_void;
    pub static kCFTypeDictionaryValueCallBacks: c_void;
}

// Helpers
pub(crate) fn cf_str(s: &str) -> CFStringRef;  // string → CFString
pub(crate) fn cf_i64(v: i64) -> *const c_void;  // i64 → CFNumber
```

---

## surface.rs

Pinned shared memory. Locked at creation, VA stable for lifetime.

```rust
pub struct Surface {
    raw: IOSurfaceRef,    // CFRelease on drop
    va: NonNull<u8>,      // stable — locked at creation
    size: usize,
    id: u32,
}

// Send + Sync — immutable after creation

impl Surface {
    /// Create pinned IOSurface. Locked immediately.
    /// ~20µs, size-independent. Errors on size=0.
    pub fn new(size: usize) -> Result<Self, MemError>;

    /// Base VA. Always valid. Inline.
    pub fn as_ptr(&self) -> *mut u8;

    /// Allocation size in bytes.
    pub fn size(&self) -> usize;

    /// Global IOSurface ID.
    pub fn id(&self) -> u32;

    /// Raw IOSurfaceRef for ANE (rane) / GPU (Metal).
    pub fn as_raw(&self) -> IOSurfaceRef;
}

impl Drop for Surface {
    fn drop(&mut self) {
        // IOSurfaceUnlock → CFRelease
    }
}
```

IOSurface properties set at creation:

| Key | Value |
|-----|-------|
| IOSurfaceWidth | size |
| IOSurfaceHeight | 1 |
| IOSurfaceBytesPerElement | 1 |
| IOSurfaceBytesPerRow | size |
| IOSurfaceAllocSize | size |
| IOSurfacePixelFormat | 0 |

---

## arena.rs

Bump allocator. ~1ns alloc. Reset in 0.3ns.

```rust
pub struct Arena {
    surface: Surface,
    cursor: AtomicUsize,
    capacity: usize,
}

// Send + Sync — atomic cursor, immutable surface

impl Arena {
    /// Create arena backed by IOSurface.
    /// ~20µs (surface creation cost).
    pub fn new(capacity: usize) -> Result<Self, MemError>;

    /// Allocate bytes. ~1ns. Lock-free compare_exchange loop.
    /// Returns None if exhausted.
    pub fn alloc(&self, size: usize, align: usize) -> Option<*mut u8>;

    /// Typed allocation.
    pub fn alloc_typed<T>(&self) -> Option<*mut T>;

    /// Reset to zero. 0.3ns. Pages stay pinned.
    pub fn reset(&self);

    /// Bytes used.
    pub fn used(&self) -> usize;

    /// Bytes remaining.
    pub fn remaining(&self) -> usize;

    /// Total capacity.
    pub fn capacity(&self) -> usize;

    /// Pointer within this arena?
    pub fn contains(&self, ptr: *const u8) -> bool;

    /// Access backing Surface.
    pub fn surface(&self) -> &Surface;
}
```

Alloc algorithm:
```rust
loop {
    current = cursor.load(Relaxed);
    aligned = (current + align - 1) & !(align - 1);
    new_cursor = aligned + size;
    if new_cursor > capacity { return None; }
    if cursor.compare_exchange_weak(current, new_cursor, Relaxed, Relaxed).is_ok() {
        return Some(surface.as_ptr() + aligned);
    }
}
```

---

## pool.rs

Fixed-size tensor pool. ~15ns acquire+release cycle.

```rust
pub struct Pool<const SLOT_SIZE: usize, const SLOTS: usize> {
    arena: Arena,
    free: SegQueue<usize>,  // crossbeam lock-free queue
}

pub struct Slot<'a> {
    ptr: *mut u8,
    index: usize,
    _pool: PhantomData<&'a ()>,  // cannot outlive Pool
}

impl<const SLOT_SIZE: usize, const SLOTS: usize> Pool<SLOT_SIZE, SLOTS> {
    /// Create pool. SLOT_SIZE must be multiple of 64.
    pub fn new() -> Result<Self, MemError>;

    /// Acquire slot. ~10ns. None if exhausted.
    pub fn acquire(&self) -> Option<Slot<'_>>;

    /// Release slot back. ~10ns.
    pub fn release(&self, slot: Slot<'_>);

    /// Free slots count.
    pub fn available(&self) -> usize;

    /// Total slots (compile-time).
    pub const fn capacity(&self) -> usize;

    /// Backing arena.
    pub fn arena(&self) -> &Arena;
}

impl<'a> Slot<'a> {
    pub fn as_ptr(&self) -> *mut u8;
    pub unsafe fn as_mut_slice(&mut self, len: usize) -> &mut [u8];
    pub fn index(&self) -> usize;
}
```

---

## Usage examples

### Basic arena allocation
```rust
use cyb_mem::Arena;

let arena = Arena::new(64 * 1024 * 1024)?; // 64MB pinned

let buf = arena.alloc(4096, 64).unwrap();
unsafe { buf.write_bytes(0, 4096); }

let tensor = arena.alloc(1 << 20, 64).unwrap(); // 1MB, 64-byte aligned
unsafe {
    let slice = std::slice::from_raw_parts_mut(tensor as *mut f32, 256 * 1024);
    slice[0] = 1.0;
}

arena.reset(); // instant — all allocations invalidated
```

### Tensor pool for inference
```rust
use cyb_mem::Pool;

// 32 slots of 4MB each = 128MB IOSurface
let pool: Pool<{ 4 * 1024 * 1024 }, 32> = Pool::new()?;

let mut slot = pool.acquire().unwrap();
unsafe {
    let data = slot.as_mut_slice(4 * 1024 * 1024);
    data[0] = 0xFF;
}
pool.release(slot);
```

### ANE integration via rane
```rust
use cyb_mem::Surface;

let surface = Surface::new(input_bytes)?;

// Write input data
unsafe {
    let ptr = surface.as_ptr() as *mut u16;
    // fill fp16 tensor...
}

// Pass to ANE — same physical memory, zero copy
let iosurface_ref = surface.as_raw();
// rane uses IOSurfaceRef directly via _ANEIOSurfaceObject
```

---

## Benchmark results

All numbers measured on Apple Silicon M1. Volatile u64, single thread.

| Operation | cyb-mem | malloc | Vec | Box | mmap | mmap+mlock |
|---|---|---|---|---|---|---|
| alloc 64 B | 1.3 ns | 16 ns | 17 ns | 15 ns | - | - |
| alloc 4 KB | 0.9 ns | 18 ns | 20 ns | 83 ns | 464 ns | - |
| alloc 1 MB | 0.9 ns | 23 ns | 22 ns | - | 461 ns | - |
| free all | 0.3 ns | ~5 ms | ~5 ms | ~5 ms | ~5 ms | ~5 ms |
| pool cycle 4 KB | 15 ns | 19 ns | 20 ns | - | 928 ns | - |
| init 16 MB (lazy) | 23 us | - | - | - | 0.47 us | - |
| init 16 MB (pretouch) | 1.4 ms | 6.7 us | - | - | - | 1.6 ms |
| write 64 MB | 23.2 GB/s | 23.1 GB/s | 23.4 GB/s | 23.4 GB/s | 22.8 GB/s | 22.7 GB/s |
| read 64 MB | 22.5 GB/s | 21.6 GB/s | 22.8 GB/s | 22.8 GB/s | 22.0 GB/s | 21.2 GB/s |
| pinned | yes | no | no | no | no | yes |
| HW shared | CPU+GPU+AMX+ANE | CPU | CPU | CPU | CPU | CPU |

Init breakdown (16 MB, one-time cost):
- cyb-mem lazy (23 us): IOSurface kernel object + DART registration + lock. Pages not backed.
- cyb-mem pretouch (1.4 ms): same + walk all 1024 pages (16KB each), trigger page faults.
- malloc touch (6.7 us): malloc reuses cached pages from system allocator — no real faults.
- mmap+mlock+touch (1.6 ms): mmap + wire + page faults. Same cost as cyb-mem pretouch.

malloc looks faster on init because the system allocator caches freed pages. On first-ever allocation (cold process), malloc+touch would be ~1.5ms too.

Bandwidth identical across all methods — DRAM bottleneck (~23 GB/s volatile u64).

cyb-mem wins on: alloc speed (15-25x), dealloc speed (16Mx), and hardware sharing (only method where CPU+GPU+AMX+ANE see one buffer without copies).
