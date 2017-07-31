use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::{thread, time};

use slog::Logger;
use reqwest;

use queue::{Queue, QueueEntry};
use api::{ApiMessage, QueuePos};
use config::Config;
use prebuffer::PreBuffer;
use broadcast::{Buffer, BufferData};
use amy;

struct RadioConn {
    tx: Sender<PreBuffer>,
}

const SYNC_AHEAD: u64 = 1;

struct Syncer {
    last_pts: f64,
    init_pts: Option<f64>,
    start: time::Instant,
}

impl Syncer {
    fn new() -> Syncer {
        Syncer {
            last_pts: 0.,
            init_pts: Some(0.),
            start: time::Instant::now(),
        }
    }

    fn update(&mut self, pts: f64) {
        if self.init_pts.is_none() {
            self.init_pts = Some(pts);
        }
        self.last_pts = pts;
    }

    fn new_song(&mut self) {
        self.start = time::Instant::now();
        self.init_pts = None;
        self.last_pts = 0.;
    }

    fn done(&mut self) {
        if let Some(dur) = time::Duration::from_millis(((self.last_pts - self.init_pts.unwrap_or(0.)) * 1000.) as u64)
            .checked_sub((time::Instant::now() - self.start)) {
            thread::sleep(dur);
        }
    }

    fn sync(&mut self) {
        if let Some(dur) = time::Duration::from_millis(((self.last_pts - self.init_pts.unwrap_or(0.)) * 1000.) as u64)
            .checked_sub(time::Duration::from_secs(SYNC_AHEAD) + (time::Instant::now() - self.start)) {
            thread::sleep(dur);
        }
    }
}

impl RadioConn {
    fn new(mid: usize,
           btx: amy::Sender<Buffer>,
           log: Logger) -> RadioConn {
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            play(rx, mid, btx, log);
        });
        RadioConn {
            tx: tx,
        }
    }

    fn replace_buffer(&mut self, buffer: PreBuffer) {
        self.tx.send(buffer).unwrap();
    }
}

pub fn play(buffer_rec: Receiver<PreBuffer>, mid: usize, btx: amy::Sender<Buffer>, log: Logger) {
    debug!(log, "Awaiting initial buffer");
    let mut pb = buffer_rec.recv().unwrap();
    let mut syncer = Syncer::new();
    loop {
        match pb.buffer.next_buf() {
            Some(BufferData::Frame { data, pts } ) => {
                syncer.update(pts);
                btx.send(Buffer::new(mid, BufferData::Frame { data, pts })).unwrap();
                syncer.sync();
            }
            Some(b @ BufferData::Header(_) ) => {
                syncer.new_song();
                btx.send(Buffer::new(mid, b)).unwrap();
            }
            Some(b @ BufferData::Trailer(_) ) => {
                btx.send(Buffer::new(mid, b)).unwrap();
            }
            None => {
                // TODO: Make this more fine grained so we know when a skip happened,
                // and apply "time gain" to account for the last page that likely didn't
                // go through
                pb.buffer.done.store(true, Ordering::Release);
                debug!(log, "Buffer drained, waiting for next!");
                pb = buffer_rec.recv().unwrap();
                debug!(log, "Received next buffer, syncing for remaining time!");
                syncer.done();
                debug!(log, "Sync complete, resuming!");
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
        // The order is guarenteed to be correct because we always iterate by the config
        // ordering.
        let tokens: Vec<_> = rconns.iter_mut().zip(prebuffers.into_iter())
            .map(|(rconn, pb)| {
                let tok = pb.buffer.done.clone();
                rconn.replace_buffer(pb);
                tok
            }).collect();

        debug!(log, "Broadcasting np");
        let np = queue.lock().unwrap().np().entry().clone();
        if let Err(e) = broadcast_np(&cfg.queue.np, np) {
            warn!(log, "Failed to broadcast np: {}", e);
        }

        queue.lock().unwrap().start_next_tc();
        debug!(log, "Entering main loop");

        // Song activity loop - ensures that the song is properly transcoding and handles any sort
        // of API message that gets received in the meanwhile
        loop {
            // If any prebuffer completes, just move on to next song. We want to minimize downtime
            // even if it means some songs get cut off early
            if tokens.iter().any(|tok| tok.load(Ordering::Acquire)) {
                break;
            } else {
                if let Ok(msg) = updates.try_recv() {
                    // Keep all these operations local just incase
                    // anything complex might need to happen in the future.
                    debug!(log, "Received API message {:?}", msg);
                    match msg {
                        ApiMessage::Skip => {
                            for token in tokens {
                                token.store(true, Ordering::Release);
                            }
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
                    thread::sleep(time::Duration::from_millis(20));
                }
            }
        }
    }
}

fn broadcast_np(url: &str, song: QueueEntry) -> Result<(), reqwest::Error> {
    let client = reqwest::Client::new()?;
    client.post(url)?
        .json(&song)?
        .send()?;
    Ok(())
}
