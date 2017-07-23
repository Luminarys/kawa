use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::{thread, io, mem};
use std::time::Duration;

use kaeru;

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
    cap: usize,
    header: Arc<Mutex<Vec<u8>>>,
    items: Arc<Mutex<Vec<u8>>>,
    writing_header: Arc<AtomicBool>,
    writing_trailer: Arc<AtomicBool>,
    write_pos: Arc<AtomicUsize>,
    read_pos: Arc<AtomicUsize>,
    len: Arc<AtomicUsize>,
}

impl RBReader {
    pub fn done(&self) -> bool {
        self.stopped() && self.rb.len() == 0
    }

    pub fn stopped(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }

    pub fn get_header(&self) -> Vec<u8> {
        mem::replace(&mut *self.rb.header.lock().unwrap(), Vec::new())
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
            let mut pos = 0;
            let bl = buf.len();
            while pos != bl {
                while self.rb.len() == 0 {
                    thread::sleep(Duration::from_millis(5));
                    if self.done() {
                        return Ok(pos);
                    }
                }

                pos += self.rb.try_read(&mut buf[pos..]);
            }
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
        self.stopped() && self.rb.len() == 0
    }

    pub fn stopped(&self) -> bool {
        self.canceled.load(Ordering::Acquire)
    }
}

impl kaeru::Sink for RBWriter {
    fn header_written(&mut self) {
        self.rb.writing_header.store(false, Ordering::Release);
    }

    fn body_written(&mut self) {
        self.rb.writing_trailer.store(true, Ordering::Release);
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
            if self.rb.writing_header.load(Ordering::Acquire) {
                let mut hb = self.rb.header.lock().unwrap();
                hb.extend(buf.iter());
                return Ok(buf.len());
            }

            let mut pos = 0;
            let bl = buf.len();
            while pos != bl {
                while self.rb.len() == self.rb.cap {
                    thread::sleep(Duration::from_millis(5));
                    if self.done() {
                        return Ok(pos);
                    }
                }

                pos += self.rb.try_write(&buf[pos..]);
            }
            Ok(bl)
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
        cap: size,
        len: Arc::new(AtomicUsize::new(0)),
        items: Arc::new(Mutex::new(vec![0u8; size])),
        header: Arc::new(Mutex::new(Vec::with_capacity(128))),
        writing_header: Arc::new(AtomicBool::new(true)),
        writing_trailer: Arc::new(AtomicBool::new(false)),
        write_pos: Arc::new(AtomicUsize::new(0)),
        read_pos: Arc::new(AtomicUsize::new(0)),
    };
    let c = Arc::new(AtomicBool::new(false));

    (RBWriter { rb: rb.clone(), canceled: c.clone() },
     RBReader { rb: rb.clone(), canceled: c.clone(), })
}


impl RingBuffer {
    pub fn try_read(&mut self, buf: &mut [u8]) -> usize {
        let l = self.len();
        let amnt = if buf.len() > l {
            self.do_read(&mut buf[..l])
        } else {
            self.do_read(buf)
        };
        self.set_len(self.len() - amnt);
        amnt
    }

    fn do_read(&mut self, buf: &mut [u8]) -> usize {
        let mut pos = 0;
        let bl = buf.len();
        let items = self.items.lock().unwrap();
        let mut rp = self.rp();
        let wp = self.wp();

        if rp >= wp {
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

    pub fn try_write(&mut self, buf: &[u8]) -> usize {
        let mr = self.cap - self.len();
        let amnt = if buf.len() > mr {
            self.do_write(&buf[..mr])
        } else {
            self.do_write(buf)
        };
        self.set_len(self.len() + amnt);
        amnt
    }

    fn do_write(&mut self, buf: &[u8]) -> usize {
        let mut pos = 0;
        let bl = buf.len();
        let mut items = self.items.lock().unwrap();
        let rp = self.rp();
        let mut wp = self.wp();

        if wp >= rp {
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
                return bl;
            } else {
                items[wp..rp].copy_from_slice(&buf[pos..(pos + rp - wp)]);
                self.set_wp(rp);
                pos += rp - wp;
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

    pub fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    pub fn set_len(&self, len: usize) {
        self.len.store(len, Ordering::Release);
    }

}

unsafe impl Send for RingBuffer { }
unsafe impl Sync for RingBuffer { }

#[test]
fn test_rb_write() {
    use std::io::Write;

    let (mut w, r) = new(10);
    w.write(&vec![1; 8]).unwrap();
    assert_eq!(w.rb.rp(), 0);
    assert_eq!(w.rb.wp(), 8);
    w.write(&vec![1; 2]).unwrap();
    assert_eq!(w.rb.wp(), 0);
}

#[test]
fn test_rb_read() {
    use std::io::{Write, Read};

    let (mut w, mut r) = new(10);
    w.write(&vec![1; 8]).unwrap();

    r.read(&mut vec![0; 5]).unwrap();
    assert_eq!(w.rb.rp(), 5);
    assert_eq!(w.rb.len(), 3);

    w.write(&vec![1; 6]).unwrap();
    assert_eq!(w.rb.wp(), 4);
    assert_eq!(w.rb.len(), 9);

    assert_eq!(w.rb.try_write(&vec![1; 5]), 1);
    assert_eq!(w.rb.wp(), w.rb.rp());
    assert_eq!(w.rb.len(), 10);

    r.read(&mut vec![0; 5]).unwrap();
    assert_eq!(w.rb.rp(), 0);
    assert_eq!(w.rb.len(), 5);
}

#[test]
fn test_conc_rw() {
    use std::io::{Write, Read};
    use std::thread;

    let (mut w, mut r) = new(100);
    let wt = thread::spawn(move || {
        for i in 0..100 {
            w.write(&vec![i; 100]).unwrap();
        }
    });
    let rt = thread::spawn(move || {
        let mut v = vec![0u8; 100];
        for i in 0..100 {
            r.read(&mut v[..33]).unwrap();
            r.read(&mut v[33..]).unwrap();
            assert_eq!(v, vec![i; 100]);
        }
    });
    wt.join();
    rt.join();
}
