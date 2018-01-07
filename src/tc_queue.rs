use std::sync::{atomic, mpsc, Arc};
use std::{mem, io, time};

use kaeru::Sink;
use broadcast::BufferData;

pub struct QW {
    queue: mpsc::SyncSender<BufferData>,
    buf: io::Cursor<Vec<u8>>,
    writing_header: bool,
    writing_trailer: bool,
    done: Arc<atomic::AtomicBool>,
}

pub struct QR {
    pub done: Arc<atomic::AtomicBool>,
    queue: mpsc::Receiver<BufferData>,
}

pub enum BufferRes {
    Data(BufferData),
    Timeout,
    Done,
}

pub fn new() -> (QW, QR) {
    let (tx, rx) = mpsc::sync_channel(15);
    let done = Arc::new(atomic::AtomicBool::new(false));
    (
        QW::new(tx, done.clone()),
        QR { queue: rx, done }
    )
}

impl QW {
    fn new(q: mpsc::SyncSender<BufferData>, done: Arc<atomic::AtomicBool>) -> QW {
        QW {
            queue: q,
            buf: io::Cursor::new(Vec::with_capacity(1024)),
            writing_header: true,
            writing_trailer: false,
            done,
        }
    }

    fn done(&self) -> bool {
        self.done.load(atomic::Ordering::Acquire)
    }
}

impl io::Write for QW {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.writing_trailer {
            self.buf.write(&buf)
        } else if self.done() {
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
            self.done.store(true, atomic::Ordering::Release);
        }
    }

    fn packet_written(&mut self, pts: f64) {
        if self.writing_trailer {
            return;
        }

        let nb = io::Cursor::new(Vec::with_capacity(1024));
        let ob = mem::replace(&mut self.buf, nb);
        let bd = BufferData::Frame {
            data: ob.into_inner(),
            pts,
        };
        if self.queue.send(bd).is_err() {
            self.done.store(true, atomic::Ordering::Release);
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
        self.done.store(true, atomic::Ordering::Release);
    }
}

impl QR {
    pub fn next_buf(&self) -> BufferRes {
        match self.queue.recv_timeout(time::Duration::from_millis(10)) {
            Ok(b) => BufferRes::Data(b),
            Err(mpsc::RecvTimeoutError::Timeout) => BufferRes::Timeout,
            Err(mpsc::RecvTimeoutError::Disconnected) => BufferRes::Done,
        }
    }
}

impl Drop for QR {
    fn drop(&mut self) {
        self.done.store(true, atomic::Ordering::Release);
    }
}
