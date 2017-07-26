use std::sync::{Arc, Mutex};
use std::{thread, mem, io, time};
use std::collections::VecDeque;

use kaeru::Sink;
use broadcast::BufferData;

pub struct QW {
    queue: Arc<Mutex<Queue>>,
    buf: io::Cursor<Vec<u8>>,
    writing_header: bool,
    writing_trailer: bool,
}

#[derive(Clone)]
pub struct QR {
    queue: Arc<Mutex<Queue>>,
}

struct Queue {
    started: bool,
    done: bool,
    data: VecDeque<BufferData>,
}

pub fn new() -> (QW, QR) {
    let q = Arc::new(Mutex::new(Queue { started: false, done: false, data: VecDeque::new() }));
    (
        QW::new(q.clone()),
        QR { queue: q }
    )
}

impl QW {
    fn new(q: Arc<Mutex<Queue>>) -> QW {
        QW {
            queue: q,
            buf: io::Cursor::new(Vec::with_capacity(1024)),
            writing_header: true,
            writing_trailer: false,
        }
    }
}

impl io::Write for QW {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.writing_header {
            loop {
                let q = self.queue.lock().unwrap();
                if q.started {
                    break;
                }
                if q.done {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Canceled!"));
                }
                drop(q);
                thread::sleep(time::Duration::from_millis(15));
            }
        } else if self.queue.lock().unwrap().done {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Canceled!"));
        }

        self.buf.write(&buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.buf.flush()
    }
}

impl Sink for QW {
    fn header_written(&mut self) {
        self.writing_header = false;
        let nb = io::Cursor::new(Vec::with_capacity(1024));
        let ob = mem::replace(&mut self.buf, nb);
        self.queue.lock().unwrap().data.push_back(BufferData::Header(ob.into_inner()));
    }

    fn packet_written(&mut self) {
        let nb = io::Cursor::new(Vec::with_capacity(1024));
        let ob = mem::replace(&mut self.buf, nb);
        self.queue.lock().unwrap().data.push_back(BufferData::Frame(ob.into_inner()));
    }

    fn body_written(&mut self) {
        let nb = io::Cursor::new(Vec::with_capacity(1024));
        let ob = mem::replace(&mut self.buf, nb);
        self.queue.lock().unwrap().data.push_back(BufferData::Frame(ob.into_inner()));
        self.writing_trailer = true;
    }
}

impl Drop for QW {
    fn drop(&mut self) {
        let mut q = self.queue.lock().unwrap();
        if self.writing_trailer {
            let nb = io::Cursor::new(Vec::with_capacity(0));
            let ob = mem::replace(&mut self.buf, nb);
            q.data.push_back(BufferData::Trailer(ob.into_inner()));
        }
        q.done = true;
    }
}

impl QR {
    pub fn start(&self) {
        self.queue.lock().unwrap().started = true;
    }

    pub fn done(&self) -> bool {
        self.queue.lock().unwrap().done
    }

    pub fn next_buf(&self) -> Option<BufferData> {
        loop {
            let mut q = self.queue.lock().unwrap();
            if q.done && q.data.len() == 0 {
                return None;
            }
            if let Some(b) = q.data.pop_front() {
                return Some(b);
            }
            drop(q);
            thread::sleep(time::Duration::from_millis(10));
        }
    }
}

impl Drop for QR {
    fn drop(&mut self) {
        self.queue.lock().unwrap().done = true;
    }
}
