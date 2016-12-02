use std::mem;
use hyper::client::Client;
use rustc_serialize::json;
use std::sync::Arc;
use std::io::Read;
use config::Config;
use prebuffer::PreBuffer;
use util;

pub struct Queue {
    pub next: Option<Vec<PreBuffer>>,
    pub entries: Vec<QueueEntry>,
    cfg: Config,
}

impl Queue {
    pub fn new(cfg: Config) -> Queue {
        Queue {
            next: None,
            entries: Vec::new(),
            cfg: cfg,
        }
    }

    pub fn push(&mut self, qe: QueueEntry) {
        self.entries.push(qe);
    }

    pub fn push_head(&mut self, qe: QueueEntry) {
        self.entries.insert(0, qe);
        self.start_next_tc();
    }

    pub fn pop(&mut self) {
        self.entries.pop();
    }

    pub fn pop_head(&mut self) {
        if !self.entries.is_empty() {
            self.entries.remove(0);
            self.start_next_tc();
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.start_next_tc();
    }

    pub fn get_next_tc(&mut self) -> Vec<PreBuffer> {
        if self.next.is_none() {
            self.start_next_tc();
        }
        let resp = mem::replace(&mut self.next, None).unwrap();
        self.start_next_tc();
        return resp;
    }

    pub fn start_next_tc(&mut self) {
        self.next = Some(self.initiate_transcode());
    }

    fn next_buffer(&mut self) -> (Arc<Vec<u8>>, String) {
        self.next_queue_buffer()
            .and(self.random_buffer())
            .unwrap_or(self.cfg.queue.fallback.clone())
    }

    fn next_queue_buffer(&mut self) -> Option<(Arc<Vec<u8>>, String)> {
        while !self.entries.is_empty() {
            {
                let entry = &self.entries[0];
                if let Some(r) = util::path_to_data(&entry.path) {
                    return Some(r);
                }
            }
            self.entries.remove(0);
        }
        return None;
    }

    fn random_buffer(&mut self) -> Option<(Arc<Vec<u8>>, String)> {
        let client = Client::new();

        let mut body = String::new();
        client.get(self.cfg.queue.random.clone())
            .send()
            .ok()
            .and_then(|mut r| r.read_to_string(&mut body).ok())
            .and_then(|_| json::decode(&body).ok())
            .and_then(|e: QueueEntry| util::path_to_data(&e.path))
    }

    fn initiate_transcode(&mut self) -> Vec<PreBuffer> {
        let (data, ext) = self.next_buffer();
        let mut prebufs = Vec::new();
        for stream in self.cfg.streams.iter() {
            if let Some(prebuf) = PreBuffer::from_transcode(data.clone(), &ext, stream) {
                prebufs.push(prebuf);
            }
        }
        prebufs
    }
}

#[derive(RustcDecodable, RustcEncodable)]
pub struct QueueEntry {
    pub id: i64,
    pub path: String,
}

impl QueueEntry {
    pub fn new(id: i64, path: String) -> QueueEntry {
        QueueEntry {
            id: id,
            path: path,
        }
    }
}
