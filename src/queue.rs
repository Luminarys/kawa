use std::{mem, fs, thread, sync};
use std::io::{self, Read};
use config::{Config, Container};
use reqwest;
use prebuffer::PreBuffer;
use ring_buffer;
use slog::Logger;
use serde_json as serde;
use kaeru;

const RB_LEN: usize = 128000;

pub struct Queue {
    pub next: Option<Vec<PreBuffer>>,
    pub entries: Vec<QueueEntry>,
    counter: usize,
    cfg: Config,
    log: Logger,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct QueueEntry {
    pub id: i64,
    pub path: String,
}

impl Queue {
    pub fn new(cfg: Config, log: Logger) -> Queue {
        Queue {
            next: None,
            entries: Vec::new(),
            cfg: cfg,
            log: log,
            counter: 0,
        }
    }

    pub fn push(&mut self, qe: QueueEntry) {
        debug!(self.log, "Inserting {:?} into queue tail!", qe);
        self.entries.push(qe);
        if self.entries.len() == 1 {
            self.start_next_tc();
        }
    }

    pub fn push_head(&mut self, qe: QueueEntry) {
        debug!(self.log, "Inserting {:?} into queue head!", qe);
        self.entries.insert(0, qe);
        self.start_next_tc();
    }

    pub fn pop(&mut self) {
        debug!(self.log, "Removing {:?} from queue tail!", self.entries.pop());
        if self.entries.len() == 0 {
            self.start_next_tc();
        }
    }

    pub fn pop_head(&mut self) {
        let res = if !self.entries.is_empty() {
            Some(self.entries.remove(0))
        } else {
            None
        };
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
        if self.next.is_none() {
            self.start_next_tc();
        }
        return mem::replace(&mut self.next, None).unwrap();
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
                self.next = Some(self.initiate_transcode(buf, ct).unwrap());
                return;
            }
            tries += 1;
            if let Some(path) = self.next_buffer() {
                match fs::File::open(path.clone()) {
                    Ok(f) => {
                        let ext = if let Some(e) = path.split('.').last() { e } else { continue };
                        match self.initiate_transcode(f, ext) {
                            Ok(bufs) => { self.next = Some(bufs); return; },
                            Err(e) => {
                                warn!(self.log, "Failed to start transcode: {}", e);
                                continue;
                            }
                        }
                    }
                    Err(e) => {
                        warn!(self.log, "Failed to open file at path {}: {}", path, e);
                        continue;
                    }
                }
            }
        }
    }

    fn next_buffer(&mut self) -> Option<String> {
        self.next_queue_buffer().or(self.random_buffer())
    }

    fn next_queue_buffer(&mut self) -> Option<String> {
        while !self.entries.is_empty() {
            let entry = &self.entries[0];
            info!(self.log, "Using queue entry {:?}", entry.path);
            return Some(entry.path.clone());
        }
        return None;
    }

    fn random_buffer(&mut self) -> Option<String> {
        let mut body = String::new();
        let res = reqwest::get(&self.cfg.queue.random.clone())
            .ok()
            .and_then(|mut r| r.read_to_string(&mut body).ok())
            .and_then(|_| serde::from_str(&body).ok())
            .map(|e: QueueEntry| {
                debug!(self.log, "Attempting to use random buffer from path {:?}", e.path);
                e.path.clone()
            });
        if res.is_some() {
            info!(self.log, "Using random entry {:?}", res);
        }
        res
    }

    fn initiate_transcode<T: io::Read + Send>(&mut self, s: T, container: &str) -> kaeru::Result<Vec<PreBuffer>> {
        let mut prebufs = Vec::new();
        let input = kaeru::Input::new(s, container)?;
        let metadata = sync::Arc::new(input.metadata());
        let mut gb = kaeru::GraphBuilder::new(input)?;
        for s in self.cfg.streams.iter() {
            let (tx, rx) = ring_buffer::new(RB_LEN);
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
        Ok(prebufs)
    }
}
