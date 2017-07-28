use std::sync::{atomic, mpsc, Arc};
use std::{thread, mem, io, time};

use kaeru::Sink;
use broadcast::BufferData;

pub struct QW {
    queue: mpsc::SyncSender<BufferData>,
    buf: io::Cursor<Vec<u8>>,
    writing_header: bool,
    writing_trailer: bool,
    done: bool,
}

pub struct QR {
    pub cancel: Arc<atomic::AtomicBool>,
    queue: mpsc::Receiver<BufferData>,
}

pub fn new() -> (QW, QR) {
    let (tx, rx) = mpsc::sync_channel(15);
    (
        QW::new(tx),
        QR { queue: rx, cancel: Arc::new(atomic::AtomicBool::new(false)) }
    )
}

impl QW {
    fn new(q: mpsc::SyncSender<BufferData>) -> QW {
        QW {
            queue: q,
            buf: io::Cursor::new(Vec::with_capacity(1024)),
            writing_header: true,
            writing_trailer: false,
            done: false,
        }
    }
}

impl io::Write for QW {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.done {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Canceled!"));
        } else {
            self.buf.write(&buf)
        }
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
        if self.queue.send(BufferData::Header(ob.into_inner())).is_err() {
            self.done = true;
        }
    }

    fn packet_written(&mut self, pts: f64) {
        let nb = io::Cursor::new(Vec::with_capacity(1024));
        let ob = mem::replace(&mut self.buf, nb);
        let bd = BufferData::Frame {
            data: ob.into_inner(),
            pts,
        };
        if self.queue.send(bd).is_err() {
            self.done = true;
        }
    }

    fn body_written(&mut self) {
        self.writing_trailer = true;
    }
}

impl Drop for QW {
    fn drop(&mut self) {
        if self.writing_trailer {
            let nb = io::Cursor::new(Vec::with_capacity(0));
            let ob = mem::replace(&mut self.buf, nb);
            if self.queue.send(BufferData::Trailer(ob.into_inner())).is_err() { }
        }
        self.done = true;
    }
}

impl QR {
    pub fn next_buf(&self) -> Option<BufferData> {
        loop {
            match self.queue.try_recv() {
                Ok(b) => return Some(b),
                Err(mpsc::TryRecvError::Empty) => {
                    thread::sleep(time::Duration::from_millis(10));
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    return None;
                }
            }
        }
    }
}

impl Drop for QR {
    fn drop(&mut self) {
        self.cancel.store(true, atomic::Ordering::Release);
    }
}
