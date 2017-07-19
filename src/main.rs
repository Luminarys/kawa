#[macro_use]
extern crate slog;
extern crate slog_term;
extern crate slog_async;
extern crate shout;
extern crate libc;
extern crate toml;
extern crate serde;
extern crate serde_json;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate serde_derive;

extern crate kaeru;

mod radio;
mod transcode;
mod config;
mod api;
mod queue;
mod util;
mod ring_buffer;
mod prebuffer;

use std::env;
use std::sync::{Arc, Mutex, mpsc};
use std::io::{Read};

lazy_static! {
    pub static ref LOG: slog::Logger = {
        let decorator = slog_term::TermDecorator::new().build();
        let drain = slog_term::FullFormat::new(decorator).build().fuse();
        let drain = slog_async::Async::new(drain).build().fuse();
        slog::Logger::root(drain, o!())
    };
}

fn main() {
    let root_log = LOG.clone();
    info!(root_log, "Initializing ffmpeg");
    if kaeru::init().is_err() {
        crit!(root_log, "FFmpeg could not be initialized!");
        return;
    }

    let path = env::args().nth(1).unwrap_or("config.toml".to_owned());
    let mut s = String::new();
    if let Ok(mut f) = std::fs::File::open(&path) {
        if f.read_to_string(&mut s).is_err() {
            crit!(root_log, "Config file could not be read!");
            return;
        }
    } else {
        crit!(root_log, "A config file path must be passed as argv[1] or must exist as ./config.toml");
        return;
    }

    info!(root_log, "Initializing config");
    let config = match config::parse_config(&s) {
        Ok(c) => c,
        Err(e) => {
            crit!(root_log, "Failed to parse config: {}", e);
            return;
        }
    };

    let api_log = root_log.new(o!("API, port" => config.api.port));
    let queue_log = root_log.new(o!("Queue" => ()));
    let radio_log = root_log.new(o!("Radio, streams" => config.streams.len()));

    let queue = Arc::new(Mutex::new(queue::Queue::new(config.clone(), queue_log)));
    let (tx, rx) = mpsc::channel();
    api::start_api(config.api.clone(), queue.clone(), tx, api_log);
    radio::start_streams(config.clone(), queue, rx, radio_log);
}
