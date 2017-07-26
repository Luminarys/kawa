use std::{mem, fs, thread, sync, time};
use std::io::{self, Read, BufReader};
use std::collections::VecDeque;
use config::{Config, Container};
use reqwest;
use prebuffer::PreBuffer;
use slog::Logger;
use serde_json as serde;
use tc_queue;
use kaeru;

// 256 KiB nuffer
const INPUT_BUF_LEN: usize = 262144;

pub struct Queue {
    entries: VecDeque<QueueEntry>,
    next: QueueBuffer,
    np: QueueBuffer,
    counter: usize,
    cfg: Config,
    log: Logger,
}

#[derive(Clone, Debug, Deserialize, Default, Serialize, PartialEq)]
pub struct QueueEntry {
    pub id: i64,
    pub path: String,
}

#[derive(Default)]
pub struct QueueBuffer {
    entry: QueueEntry,
    dur: time::Duration,
    bufs: Vec<PreBuffer>,
}

impl Queue {
    pub fn new(cfg: Config, log: Logger) -> Queue {
        let mut q = Queue {
            np: Default::default(),
            next: Default::default(),
            entries: VecDeque::new(),
            cfg: cfg,
            log: log,
            counter: 0,
        };
        q.start_next_tc();
        q
    }

    pub fn np(&self) -> &QueueBuffer {
        &self.np
    }

    pub fn entries(&self) -> &VecDeque<QueueEntry> {
        &self.entries
    }

    pub fn push(&mut self, qe: QueueEntry) {
        debug!(self.log, "Inserting {:?} into queue tail!", qe);
        self.entries.push_back(qe);
        if self.entries.len() == 1 {
            self.start_next_tc();
        }
    }

    pub fn push_head(&mut self, qe: QueueEntry) {
        debug!(self.log, "Inserting {:?} into queue head!", qe);
        self.entries.push_front(qe);
        self.start_next_tc();
    }

    pub fn pop(&mut self) {
        debug!(self.log, "Removing {:?} from queue tail!", self.entries.pop_back());
        if self.entries.len() == 0 {
            self.start_next_tc();
        }
    }

    pub fn pop_head(&mut self) {
        let res = self.entries.pop_front();
        debug!(self.log, "Removing {:?} from queue head!", res);
        self.start_next_tc();
    }

    pub fn clear(&mut self) {
        debug!(self.log, "Clearing queue!");
        self.entries.clear();
        self.start_next_tc();
    }

    pub fn get_next_tc(&mut self) -> Vec<PreBuffer> {
        debug!(self.log, "Extracting current pre-transcode!");
        // Swap next into np, then clear next and extract np buffers
        mem::swap(&mut self.next, &mut self.np);
        self.next = Default::default();
        // Pop queue head if its the same as np
        if self.entries.front().map(|e| *e == self.np.entry).unwrap_or(false) {
            self.pop_head();
        }
        mem::replace(&mut self.np.bufs, Vec::new())
    }

    pub fn start_next_tc(&mut self) {
        debug!(self.log, "Beginning next pre-transcode!");
        let mut tries = 0;
        loop {
            if tries == 5 {
                use std::borrow::Borrow;
                let buf = {
                    let b: &Vec<u8> = self.cfg.queue.fallback.0.borrow();
                    io::Cursor::new(b.clone())
                };
                // TODO: Make this less retarded - Rust can't deal with two levels of dereference
                let ct = &self.cfg.queue.fallback.1.clone();
                warn!(self.log, "Using fallback");
                let tc = self.initiate_transcode(buf, ct).unwrap();
                self.next = QueueBuffer {
                    dur: tc.0,
                    bufs: tc.1,
                    entry: QueueEntry { id: 0, path: "fallback".to_owned() },
                };
                return;
            }
            tries += 1;
            if let Some(qe) = self.next_buffer() {
                match fs::File::open(&qe.path) {
                    Ok(f) => {
                        let ext = if let Some(e) = qe.path.split('.').last() { e } else { continue };
                        match self.initiate_transcode(f, ext) {
                            Ok(tc) => {
                                self.next = QueueBuffer {
                                    dur: tc.0,
                                    bufs: tc.1,
                                    entry: qe.clone(),
                                };
                                return;
                            },
                            Err(e) => {
                                warn!(self.log, "Failed to start transcode: {}", e);
                                continue;
                            }
                        }
                    }
                    Err(e) => {
                        warn!(self.log, "Failed to open queue entry {:?}: {}", qe, e);
                        continue;
                    }
                }
            }
        }
    }

    fn next_buffer(&mut self) -> Option<QueueEntry> {
        self.next_queue_buffer().or_else(|| self.random_buffer())
    }

    fn next_queue_buffer(&mut self) -> Option<QueueEntry> {
        let e = self.entries.front().cloned();
        if let Some(ref er) = e {
            info!(self.log, "Using queue entry {:?}", er);
        }
        e
    }

    fn random_buffer(&mut self) -> Option<QueueEntry> {
        let mut body = String::new();
        let res = reqwest::get(&self.cfg.queue.random.clone())
            .ok()
            .and_then(|mut r| r.read_to_string(&mut body).ok())
            .and_then(|_| serde::from_str(&body).ok());
        if res.is_some() {
            info!(self.log, "Using random entry {:?}", res.as_ref().unwrap());
        }
        res
    }

    fn initiate_transcode<T: io::Read + Send>(&mut self, s: T, container: &str) -> kaeru::Result<(time::Duration, Vec<PreBuffer>)> {
        let mut prebufs = Vec::new();
        let input = kaeru::Input::new(BufReader::with_capacity(INPUT_BUF_LEN, s), container)?;
        let dur = input.duration();
        let metadata = sync::Arc::new(input.metadata());
        let mut gb = kaeru::GraphBuilder::new(input)?;
        for s in self.cfg.streams.iter() {
            let (tx, rx) = tc_queue::new();
            let ct = match s.container {
                Container::Ogg => "ogg",
                Container::MP3 => "mp3",
            };
            let output = kaeru::Output::new(tx, ct, s.codec, s.bitrate)?;
            gb.add_output(output)?;
            let log = self.log.new(o!("Transcoder, mount" => s.mount.clone(), "QID" => self.counter));
            prebufs.push(PreBuffer::new(rx, metadata.clone(), log));
        }
        let g = gb.build()?;
        let log = self.log.new(o!("QID" => self.counter, "thread" => "transcoder"));
        thread::spawn(move || {
            debug!(log, "Starting");
            match g.run() {
                Ok(()) => { }
                Err(e) => { debug!(log, "completed with err: {}", e) }
            }
            debug!(log, "Completed");
        });
        self.counter += 1;
        Ok((dur, prebufs))
    }
}

impl QueueBuffer {
    pub fn entry(&self) -> &QueueEntry {
        &self.entry
    }

    pub fn duration(&self) -> &time::Duration {
        &self.dur
    }
}
