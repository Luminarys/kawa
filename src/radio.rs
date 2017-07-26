use std::sync::{Arc, Mutex};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Instant, Duration};
use slog::Logger;

use queue::Queue;
use api::{ApiMessage, QueuePos};
use config::Config;
use prebuffer::PreBuffer;
use broadcast::Buffer;
use amy;

struct RadioConn {
    tx: Sender<PreBuffer>,
    handle: thread::JoinHandle<()>,
}

impl RadioConn {
    fn new(mid: usize,
           btx: amy::Sender<Buffer>,
           log: Logger) -> RadioConn {
        let (tx, rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            play(rx, mid, btx, log);
        });
        RadioConn {
            tx: tx,
            handle: handle,
        }
    }

    fn replace_buffer(&mut self, buffer: PreBuffer) {
        self.tx.send(buffer).unwrap();
    }
}

pub fn play(buffer_rec: Receiver<PreBuffer>, mid: usize, btx: amy::Sender<Buffer>, log: Logger) {
    debug!(log, "Awaiting initial buffer");
    let mut pb = buffer_rec.recv().unwrap();
    pb.buffer.start();
    loop {
        match pb.buffer.next_buf() {
            Some(b) => {
                btx.send(Buffer::new(mid, b)).unwrap();
            }
            None => {
                debug!(log, "Buffer drained, waiting for next!");
                pb = buffer_rec.recv().unwrap();
                pb.buffer.start();
            }
        }
    }
}

pub fn start_streams(cfg: Config,
                     queue: Arc<Mutex<Queue>>,
                     updates: Receiver<ApiMessage>,
                     btx: amy::Sender<Buffer>,
                     log: Logger,) {
    let mut rconns: Vec<_> = cfg.streams.iter().enumerate()
        .map(|(id, stream)| {
            let rlog = log.new(o!("mount" => stream.mount.clone()));
            RadioConn::new(id,
                             btx.try_clone().unwrap(),
                             rlog)
        })
        .collect();

    loop {
        debug!(log, "Extracting next buffer");
        let prebuffers = queue.lock().unwrap().get_next_tc();

        debug!(log, "Dispatching new buffers");
        for (rconn, pb) in rconns.iter_mut().zip(prebuffers.iter()) {
            // The order is guarenteed to be correct because we always iterate by the config
            // ordering.
            rconn.replace_buffer(pb.clone());
        }
        let end_time = Instant::now() + *queue.lock().unwrap().np().duration();

        debug!(log, "Entering main loop");

        // Song activity loop - ensures that the song is properly transcoding and handles any sort
        // of API message that gets received in the meanwhile
        loop {
            // If any prebuffer completes, just move on to next song. We want to minimize downtime
            // even if it means some songs get cut off early
            if prebuffers.iter().any(|pb| pb.buffer.done()) {
                let now = Instant::now();
                // Protection in case duration gives a non sane value
                if end_time > now && end_time - now < Duration::from_secs(10) {
                    debug!(log, "Sleeping for {:?} to make up for difference in duration", end_time - now);
                    thread::sleep(end_time - now);
                }
                break;
            } else {
                if let Ok(msg) = updates.try_recv() {
                    // Keep all these operations local just incase
                    // anything complex might need to happen in the future.
                    debug!(log, "Received API message {:?}", msg);
                    match msg {
                        ApiMessage::Skip => {
                            break;
                        }
                        ApiMessage::Clear => {
                            queue.lock().unwrap().clear();
                        }
                        ApiMessage::Insert(QueuePos::Head, qe) => {
                            queue.lock().unwrap().push_head(qe);
                        }
                        ApiMessage::Insert(QueuePos::Tail, qe) => {
                            queue.lock().unwrap().push(qe);
                        }
                        ApiMessage::Remove(QueuePos::Head) => {
                            queue.lock().unwrap().pop_head();
                        }
                        ApiMessage::Remove(QueuePos::Tail) => {
                            queue.lock().unwrap().pop();
                        }
                    }
                } else {
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
}
