extern crate shout;
extern crate ffmpeg;
extern crate libc;
extern crate hyper;
extern crate toml;

mod radio;
mod transcode;
mod config;
mod api;

use std::env;

use std::sync::{Arc, Mutex};
use std::io::{Read};

pub type Queue = Vec<(String, String)>;

fn main() {
    ffmpeg::init().unwrap();

    let path = env::args().nth(1).expect("missing config");
    let mut s = String::new();
    let mut f = std::fs::File::open(&path).expect("invalid file");
    f.read_to_string(&mut s);
    let config = match config::parse_config(s) {
        Ok(c) => c,
        Err(e) => {
            println!("{}", e);
            return
        }
    };

    let queue = Arc::new(Mutex::new(Queue::new()));
    if let Some(path) = env::args().nth(2) {
        queue.lock().unwrap().push(("".to_owned(), path));
    }
    api::start_api(config.api.clone(), queue.clone());
    radio::start_streams(config.radio.clone(), config.streams.clone(), queue);
}
