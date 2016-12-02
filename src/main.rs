extern crate shout;
extern crate ffmpeg;
extern crate libc;
extern crate hyper;
extern crate toml;
extern crate rustc_serialize;
extern crate ring_buffer;

#[macro_use]
extern crate rustful;

mod radio;
mod transcode;
mod config;
mod api;
mod queue;

use std::env;
use std::sync::{Arc, Mutex, mpsc};
use std::io::{Read};

fn main() {
    ffmpeg::init().unwrap();

    let path = env::args().nth(1).expect("missing config");
    let mut s = String::new();
    let mut f = std::fs::File::open(&path).expect("invalid file");
    f.read_to_string(&mut s).unwrap();
    let config = match config::parse_config(s) {
        Ok(c) => c,
        Err(e) => {
            println!("Failed to parse config: {}", e);
            return
        }
    };

    let queue = Arc::new(Mutex::new(queue::Queue::default()));
    if let Some(path) = env::args().nth(2) {
        queue.lock().unwrap().entries.push(queue::QueueEntry::new(0, path));
    }
    let (tx, rx) = mpsc::channel();
    api::start_api(config.api.clone(), queue.clone(), tx);
    radio::start_streams(config.clone(), queue, rx);
}
