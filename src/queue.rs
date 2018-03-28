use std::{mem, fs, thread, sync};
use std::io::{self, Read, BufReader};
use std::collections::VecDeque;
use config::{Config, Container};
use reqwest;
use prebuffer::PreBuffer;
use serde_json as serde;
use serde_json::Map;
use serde_json::Value as JSON;
use tc_queue;
use kaeru;

// 256 KiB nuffer
const INPUT_BUF_LEN: usize = 262144;

pub struct Queue {
    entries: VecDeque<QueueEntry>,
    next: QueueBuffer,
    np: QueueBuffer,
    counter: u64,
    last_id: u64,
    cfg: Config,
}

#[derive(Clone, Debug, Deserialize, Default, PartialEq)]
pub struct NewQueueEntry {
    pub data: Map<String, JSON>,
    pub path: String,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct QueueEntry {
    pub id: u64,
    pub data: Map<String, JSON>,
    pub path: String,
}

#[derive(Default)]
pub struct QueueBuffer {
    entry: QueueEntry,
    bufs: Vec<PreBuffer>,
}

impl Queue {
    pub fn new(cfg: Config) -> Queue {
        let mut q = Queue {
            np: Default::default(),
            next: Default::default(),
            entries: VecDeque::new(),
            cfg: cfg,
            counter: 0,
            last_id: 0,
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

    pub fn push(&mut self, nqe: NewQueueEntry) {
        debug!("Inserting {:?} into queue tail!", nqe);
        let qe = self.queue_entry_from_new(nqe);
        self.entries.push_back(qe);
        if self.entries.len() == 1 {
            self.start_next_tc();
        }
    }

    pub fn push_head(&mut self, nqe: NewQueueEntry) {
        debug!("Inserting {:?} into queue head!", nqe);
        let qe = self.queue_entry_from_new(nqe);
        self.entries.push_front(qe);
        self.start_next_tc();
    }

    pub fn pop(&mut self) {
        let entry = self.entries.pop_back();
        debug!("Removing {:?} from queue tail!", entry);
        if self.entries.is_empty() {
            self.start_next_tc();
        }
    }

    pub fn pop_head(&mut self) {
        let res = self.entries.pop_front();
        debug!("Removing {:?} from queue head!", res);
        self.start_next_tc();
    }

    pub fn clear(&mut self) {
        debug!("Clearing queue!");
        if !self.entries.is_empty() {
            self.entries.clear();
            self.start_next_tc();
        }
    }

    pub fn get_next_tc(&mut self) -> Vec<PreBuffer> {
        debug!("Extracting current pre-transcode!");
        // Swap next into np, then clear next and extract np buffers
        mem::swap(&mut self.next, &mut self.np);
        self.next = Default::default();
        // Pop queue head if its the same as np, and start next transcode
        if self.entries.front().map(|e| *e == self.np.entry).unwrap_or(false) {
            self.entries.pop_front();
        }
        mem::replace(&mut self.np.bufs, Vec::new())
    }

    pub fn start_next_tc(&mut self) {
        debug!("Beginning next pre-transcode!");
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
                warn!("Using fallback");
                let tc = self.initiate_transcode(buf, ct).unwrap();
                self.next = QueueBuffer {
                    bufs: tc,
                    entry: self.queue_entry_from_new(NewQueueEntry { data: Map::new(), path: "fallback".to_owned() }),
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
                                    bufs: tc,
                                    entry: qe.clone(),
                                };
                                return;
                            },
                            Err(e) => {
                                warn!("Failed to start transcode: {}", e);
                                continue;
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to open queue entry {:?}: {}", qe, e);
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
            info!("Using queue entry {:?}", er);
        }
        e
    }

    fn random_buffer(&mut self) -> Option<QueueEntry> {
        let mut body = String::new();
        let res = reqwest::get(&self.cfg.queue.random.clone())
            .ok()
            .and_then(|mut r| r.read_to_string(&mut body).ok())
            .and_then(|_| serde::from_str(&body).ok())
            .and_then(|v| NewQueueEntry::deserialize(v))
            .map(|v| self.queue_entry_from_new(v));
        if res.is_some() {
            info!("Using random entry {:?}", res.as_ref().unwrap());
        }
        res
    }

    fn initiate_transcode<T: io::Read + Send>(&mut self, s: T, container: &str) -> kaeru::Result<Vec<PreBuffer>> {
        let mut prebufs = Vec::new();
        let input = kaeru::Input::new(BufReader::with_capacity(INPUT_BUF_LEN, s), container)?;
        let metadata = sync::Arc::new(input.metadata());
        let mut gb = kaeru::GraphBuilder::new(input)?;
        for s in self.cfg.streams.iter() {
            let (tx, rx) = tc_queue::new();
            let ct = match s.container {
                Container::Ogg => "ogg",
                Container::MP3 => "mp3",
                Container::FLAC => "flac",
            };
            let output = kaeru::Output::new(tx, ct, s.codec, s.bitrate)?;
            gb.add_output(output)?;
            prebufs.push(PreBuffer::new(rx, metadata.clone()));
        }
        let g = gb.build()?;
        thread::spawn(move || {
            debug!("Starting transcode");
            match g.run() {
                Ok(()) => { }
                Err(e) => { debug!("transcode completed with err: {}", e) }
            }
            debug!("Completed transcode");
        });
        self.counter += 1;
        Ok(prebufs)
    }

    fn queue_entry_from_new(&mut self, nqe: NewQueueEntry) -> QueueEntry {
        self.last_id += 1;
        QueueEntry { id: self.last_id, data: nqe.data, path: nqe.path }
    }
}

impl NewQueueEntry {
    pub fn deserialize(json: JSON) -> Option<NewQueueEntry> {
        match json {
            JSON::Object(o) => {
                match o.get("path").cloned() {
                    Some(JSON::String(p)) => Some(NewQueueEntry { data: o, path: p }),
                    _ => None,
                }
            }
            _ => None
        }
    }
}

impl QueueEntry {
    pub fn serialize(&self) -> JSON {
        JSON::Object(self.data.clone())
    }
}

impl QueueBuffer {
    pub fn entry(&self) -> &QueueEntry {
        &self.entry
    }
}
