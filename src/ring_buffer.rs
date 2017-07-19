use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::{mem, thread};
use std::time::Duration;

pub struct RingBuffer {
    items: Arc<Mutex<<Vec<u8>>>>,
    write_pos: AtomicUsize,
    read_pos: AtomicUsize,
}

impl<T> RingBuffer<T> {
    pub fn new(size: usize) -> RingBuffer<T> {
        let items = (0..size).map(|_| None).collect();
        RingBuffer {
            size: size,
            items: UnsafeCell::new(items),
            write_pos: AtomicUsize::new(0),
            read_pos: AtomicUsize::new(0),
        }
    }

    pub fn push(&self, item: T) {
        let write_pos = self.write_pos.load(Ordering::Acquire);
        loop {
            let read_pos = self.read_pos.load(Ordering::Acquire);
            if write_pos - read_pos != self.size {
                break;
            } else {
                thread::park_timeout(Duration::from_millis(10));
            }
        }

        unsafe {
            let mut items = &mut *self.items.get();
            mem::replace(&mut items[write_pos % self.size], Some(item));
        }
        self.write_pos.store(write_pos + 1, Ordering::Release);
    }

    pub fn pop(&self) -> T {
        let read_pos = self.read_pos.load(Ordering::Acquire);
        loop {
            let write_pos = self.write_pos.load(Ordering::Acquire);
            if write_pos != read_pos {
                break;
            } else {
                thread::park_timeout(Duration::from_millis(10));
            }
        }

        let item = unsafe {
            let mut items = &mut *self.items.get();
            mem::replace(&mut items[read_pos % self.size], None)
        };
        self.read_pos.store(read_pos + 1, Ordering::Release);
        item.unwrap()
    }

    pub fn read(&mut self, buf: &mut [u8]) {
        let mut pos = 0;
        let bl = buf.len();
        while pos != bl {
            // Sleep until we have data
            while self.rp() != self.wp() {
                thread::sleep(Duration::from_millis(5));
            }

            let items = self.items.lock().unwrap();
            let mut rp = self.rp();
            let wp = self.wp();
            let mut left = bl - pos;

            if rp > wp {
                if left < items.len() - rp {
                    buf[pos..].copy_from_slice(&items[rp..(rp + left)]);
                    self.set_rp(rp + left);
                    return;
                } else {
                    let read = items.len() - rp;
                    buf[pos..(pos + read)].copy_from_slice(&items[rp..]);
                    pos += read;
                    self.set_rp(0);
                    rp = 0;
                }
            }

            left = bl - pos;

            if rp < wp {
                if left < wp - rp {
                    buf[pos..].copy_from_slice(&items[rp..(rp + left)]);
                    self.set_rp(rp + left);
                    return;
                } else {
                    buf[pos..(pos + wp - rp)].copy_from_slice(&items[rp..wp]);
                    pos += wp - rp;
                    self.set_rp(wp);
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn write(&mut self, buf: &[u8]) {
        let mut pos = 0;
        let bl = buf.len();
        while pos != bl {
            while self.wp() != self.rp() {
                thread::sleep(Duration::from_millis(5));
            }

            let items = self.items.lock().unwrap();
            let rp = self.rp();
            let mut wp = self.wp();
            let mut left = bl - pos;

            if wp > rp {
                if left < items.len() - wp {
                    self.items[wp..(wp + left)].copy_from_slice(&buf[pos..]);
                    self.set_wp(wp + left);
                    return;
                } else {
                    self.items[wp..].copy_from_slice(&buf[pos..]);
                    pos += items.len() - wp;
                    selt.set_wp(0);
                    wp = 0;
                }
            }

            left = bl - pos;

            if wp < rp {
                if left < rp - wp {
                    self.items[wp..(wp + left)].copy_from_slice(&buf[pos..]);
                    self.set_wp(wp + left);
                    return;
                } else {
                    self.items[wp..rp].copy_from_slice(&buf[pos..(pos + rp - wp)]);
                    self.set_wp(rp);
                }
            }
        }
    }

    fn rp(&self) -> usize {
        self.read_pos.load(Ordering::Acquire)
    }

    fn set_rp(&self, rp: usize) {
        self.read_pos.store(Ordering::Release)
    }

    fn wp(&self) -> usize {
        self.write_pos.load(Ordering::Acquire)
    }

    fn set_wp(&self, rp: usize) {
        self.write_pos.store(Ordering::Release)
    }
}
