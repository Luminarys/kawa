#[macro_use]
extern crate slog;
extern crate slog_term;
extern crate slog_async;
extern crate shout;
extern crate libc;
extern crate toml;
extern crate serde;
extern crate serde_json;
extern crate reqwest;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate rouille;

extern crate kaeru;

mod radio;
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
        use slog::Drain;

        let decorator = slog_term::TermDecorator::new().build();
        let drain = slog_term::FullFormat::new(decorator).build().fuse();
        let drain = slog_async::Async::new(drain).build().fuse();
        slog::Logger::root(drain, o!())
    };
}

fn main() {
    let root_log = LOG.clone();
    info!(root_log, "Initializing ffmpeg");
    kaeru::init();
    // if kaeru::init().is_err() {
    //     crit!(root_log, "FFmpeg could not be initialized!");
    //     return;
    // }

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

#[cfg(test)]
mod tests {
    use super::*;
    use super::kaeru::{Input, Output, GraphBuilder};
    use std::thread;
    use std::fs::File;

    #[test]
    fn test_tc() {
        kaeru::init();
        tc().unwrap()
    }

    fn tc() -> kaeru::Result<()> {
        let fin = File::open("/tmp/in.flac").unwrap();
        let (w1, mut r1) = ring_buffer::new(4096 * 2);
        let (w2, mut r2) = ring_buffer::new(4096 * 2);
        let (w3, mut r3) = ring_buffer::new(4096 * 2);
    
        let i = Input::new(fin, "flac")?;
        let o1 = Output::new(w2, "mp3", kaeru::AVCodecID::AV_CODEC_ID_MP3, Some(192))?;
        let o2 = Output::new(w1, "ogg", kaeru::AVCodecID::AV_CODEC_ID_OPUS, Some(192))?;
        let o3 = Output::new(w3, "ogg", kaeru::AVCodecID::AV_CODEC_ID_FLAC, None)?;
        let mut gb = GraphBuilder::new(i)?;
        gb.add_output(o1)?.add_output(o2)?.add_output(o3)?;
        let g = gb.build()?;
        let gt = thread::spawn(move || g.run().unwrap());
        let t1 = thread::spawn(move || {
            let mut buf = vec![0u8; 4096];
            while !r1.done() {
                let a = r1.read(&mut buf).unwrap();
            }
        });
        let t2 = thread::spawn(move || {
            let mut buf = vec![0u8; 4096];
            while !r2.done() {
                let a = r2.read(&mut buf).unwrap();
            }
        });
        let t3 = thread::spawn(move || {
            let mut buf = vec![0u8; 4096];
            while !r3.done() {
                let a = r3.read(&mut buf).unwrap();
            }
        });
        gt.join().unwrap();
        t1.join().unwrap();
        t2.join().unwrap();
        t3.join().unwrap();
        Ok(())
    }
}
