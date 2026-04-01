use unimem::{Block, Grid, Tape};

#[test]
fn block_open_and_access() {
    let b = Block::open(4096).unwrap();
    assert!(b.size() >= 4096);
    assert!(!b.address().is_null());
    assert!(b.id() > 0);

    unsafe {
        let ptr = b.address();
        ptr.write_bytes(0xAB, 4096);
        assert_eq!(*ptr, 0xAB);
        assert_eq!(*ptr.add(4095), 0xAB);
    }
}

#[test]
fn block_zero_size_errors() {
    assert!(Block::open(0).is_err());
}

#[test]
fn block_large() {
    let b = Block::open(256 * 1024 * 1024).unwrap();
    assert!(b.size() >= 256 * 1024 * 1024);
    unsafe {
        b.address().write(42);
        b.address().add(b.size() - 1).write(99);
        assert_eq!(*b.address(), 42);
        assert_eq!(*b.address().add(b.size() - 1), 99);
    }
}

#[test]
fn tape_basic_take() {
    let tape = Tape::start(1024 * 1024).unwrap();
    assert_eq!(tape.used(), 0);

    let p1 = tape.take(256, 64).unwrap();
    assert_eq!(tape.used(), 256);
    assert!(tape.owns(p1));

    let p2 = tape.take(512, 64).unwrap();
    assert!(tape.used() >= 768);
    assert!(tape.owns(p2));
    assert_ne!(p1, p2);

    unsafe {
        p1.write_bytes(0x11, 256);
        p2.write_bytes(0x22, 512);
        assert_eq!(*p1, 0x11);
        assert_eq!(*p2, 0x22);
    }
}

#[test]
fn tape_clear() {
    let tape = Tape::start(4096).unwrap();
    let _ = tape.take(1000, 1).unwrap();
    assert!(tape.used() >= 1000);

    tape.clear();
    assert_eq!(tape.used(), 0);
    assert_eq!(tape.free(), tape.total());

    let p = tape.take(2000, 1).unwrap();
    assert!(!p.is_null());
}

#[test]
fn tape_full() {
    let tape = Tape::start(4096).unwrap();
    let cap = tape.total();
    let _ = tape.take(cap, 1).unwrap();
    assert_eq!(tape.free(), 0);
    assert!(tape.take(1, 1).is_none());
}

#[test]
fn tape_alignment() {
    let tape = Tape::start(1024 * 1024).unwrap();
    let _ = tape.take(1, 1).unwrap();
    let p = tape.take(64, 64).unwrap();
    assert_eq!(p as usize % 64, 0);

    let p2 = tape.take(4096, 4096).unwrap();
    assert_eq!(p2 as usize % 4096, 0);
}

#[test]
fn tape_owns() {
    let tape = Tape::start(4096).unwrap();
    let p = tape.take(100, 1).unwrap();
    assert!(tape.owns(p));
    assert!(tape.owns(unsafe { p.add(99) }));
    assert!(!tape.owns(std::ptr::null()));
    assert!(!tape.owns(0x1 as *const u8));
}

#[test]
fn tape_take_one() {
    let tape = Tape::start(4096).unwrap();
    let p: *mut u64 = tape.take_one::<u64>().unwrap();
    assert_eq!(p as usize % std::mem::align_of::<u64>(), 0);
    unsafe {
        p.write(0xDEADBEEF_CAFEBABE);
        assert_eq!(p.read(), 0xDEADBEEF_CAFEBABE);
    }
}

#[test]
fn grid_take_give() {
    let grid: Grid<256, 4> = Grid::new().unwrap();
    assert_eq!(grid.free(), 4);
    assert_eq!(grid.total(), 4);

    let c0 = grid.take().unwrap();
    let c1 = grid.take().unwrap();
    assert_eq!(grid.free(), 2);
    assert_ne!(c0.address(), c1.address());

    unsafe {
        c0.address().write_bytes(0xAA, 256);
        c1.address().write_bytes(0xBB, 256);
        assert_eq!(*c0.address(), 0xAA);
        assert_eq!(*c1.address(), 0xBB);
    }

    grid.give(c0);
    assert_eq!(grid.free(), 3);
    grid.give(c1);
    assert_eq!(grid.free(), 4);
}

#[test]
fn grid_full() {
    let grid: Grid<64, 2> = Grid::new().unwrap();
    let _c0 = grid.take().unwrap();
    let _c1 = grid.take().unwrap();
    assert!(grid.take().is_none());
}

#[test]
fn grid_reuse() {
    let grid: Grid<64, 1> = Grid::new().unwrap();
    let cell = grid.take().unwrap();
    let addr = cell.address();
    grid.give(cell);

    let cell2 = grid.take().unwrap();
    assert_eq!(cell2.address(), addr);
    grid.give(cell2);
}
