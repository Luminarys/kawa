use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::{thread, io};
use std::time::Duration;

#[derive(Clone)]
pub struct RBReader {
    rb: RingBuffer,
    canceled: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct RBWriter {
    rb: RingBuffer,
    canceled: Arc<AtomicBool>,
}

#[derive(Clone)]
struct RingBuffer {
    items: Arc<Mutex<Vec<u8>>>,
    write_pos: Arc<AtomicUsize>,
    read_pos: Arc<AtomicUsize>,
}

impl RBReader {
    pub fn done(&self) -> bool {
        self.stopped() && self.rb.wp() == self.rb.rp()
    }

    pub fn stopped(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }
}

impl io::Read for RBReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.canceled.load(Ordering::Acquire) {
            match self.rb.try_read(buf) {
                0 => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Canceled!")),
                a => Ok(a)
            }
        } else {
            self.rb.read(buf);
            Ok(buf.len())
        }
    }
}

unsafe impl Send for RBReader { }

impl Drop for RBReader {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

#[allow(dead_code)]
impl RBWriter {
    pub fn done(&self) -> bool {
        self.stopped() && self.rb.wp() == self.rb.rp()
    }

    pub fn stopped(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }
}

impl io::Write for RBWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.canceled.load(Ordering::Acquire) {
            match self.rb.try_write(buf) {
                0 => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Canceled!")),
                a => Ok(a)
            }
        } else {
            self.rb.write(buf);
            Ok(buf.len())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for RBWriter {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Release);
    }
}

unsafe impl Send for RBWriter { }

pub fn new(size: usize) -> (RBWriter, RBReader) {
    let rb = RingBuffer {
        items: Arc::new(Mutex::new(vec![0u8; size])),
        write_pos: Arc::new(AtomicUsize::new(0)),
        read_pos: Arc::new(AtomicUsize::new(0)),
    };
    let c = Arc::new(AtomicBool::new(false));

    (RBWriter { rb: rb.clone(), canceled: c.clone(), },
     RBReader { rb: rb.clone(), canceled: c.clone(), })
}


impl RingBuffer {
    pub fn read(&mut self, buf: &mut [u8]) {
        let mut pos = 0;
        let bl = buf.len();
        while pos != bl {
            while self.rp() != self.wp() {
                thread::sleep(Duration::from_millis(5));
            }
            pos += self.try_read(&mut buf[pos..]);
        }
    }

    pub fn try_read(&mut self, buf: &mut [u8]) -> usize {
        let mut pos = 0;
        let bl = buf.len();
        let items = self.items.lock().unwrap();
        let mut rp = self.rp();
        let wp = self.wp();

        if rp > wp {
            if bl < items.len() - rp {
                buf[..].copy_from_slice(&items[rp..(rp + bl)]);
                self.set_rp(rp + bl);
                return bl;
            } else {
                let read = items.len() - rp;
                buf[pos..(pos + read)].copy_from_slice(&items[rp..]);
                pos += read;
                self.set_rp(0);
                rp = 0;
            }
        }

        let left = bl - pos;

        if rp < wp {
            if left < wp - rp {
                buf[pos..].copy_from_slice(&items[rp..(rp + left)]);
                self.set_rp(rp + left);
                return bl;
            } else {
                buf[pos..(pos + wp - rp)].copy_from_slice(&items[rp..wp]);
                pos += wp - rp;
                self.set_rp(wp);
            }
        }
        return pos;
    }

    pub fn write(&mut self, buf: &[u8]) {
        let mut pos = 0;
        while pos != buf.len() {
            while self.wp() != self.rp() {
                thread::sleep(Duration::from_millis(5));
            }
            pos += self.try_write(&buf[pos..]);
        }
    }

    pub fn try_write(&mut self, buf: &[u8]) -> usize {
        let mut pos = 0;
        let bl = buf.len();
        let mut items = self.items.lock().unwrap();
        let rp = self.rp();
        let mut wp = self.wp();

        if wp > rp {
            if bl < items.len() - wp {
                items[wp..(wp + bl)].copy_from_slice(buf);
                self.set_wp(wp + bl);
                return bl;
            } else {
                let il = items.len();
                items[wp..].copy_from_slice(&buf[..(il - wp)]);
                pos += items.len() - wp;
                self.set_wp(0);
                wp = 0;
            }
        }

        let left = bl - pos;

        if wp < rp {
            if left < rp - wp {
                items[wp..(wp + left)].copy_from_slice(&buf[pos..]);
                self.set_wp(wp + left);
            } else {
                items[wp..rp].copy_from_slice(&buf[pos..(pos + rp - wp)]);
                self.set_wp(rp);
            }
        }
        return pos;
    }

    fn rp(&self) -> usize {
        self.read_pos.load(Ordering::Acquire)
    }

    fn set_rp(&self, rp: usize) {
        self.read_pos.store(rp, Ordering::Release)
    }

    fn wp(&self) -> usize {
        self.write_pos.load(Ordering::Acquire)
    }

    fn set_wp(&self, wp: usize) {
        self.write_pos.store(wp, Ordering::Release)
    }
}

unsafe impl Send for RingBuffer { }
unsafe impl Sync for RingBuffer { }
