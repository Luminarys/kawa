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
use std::thread;

fn main() {
    ffmpeg::init().unwrap();
    let path = env::args().nth(1).expect("missing file");
    let mut f = std::fs::File::open(&path).expect("invalid file");
    let mut in_buf = Vec::new();
    let ext = std::path::Path::new(&path).extension().unwrap();
    f.read_to_end(&mut in_buf).unwrap();
    let in_buf = Arc::new(in_buf);

    let radio_endpoints = vec![("/test.flac".to_owned(), shout::ShoutFormat::Ogg), ("/test.mp3".to_owned(), shout::ShoutFormat::MP3)];
    let buffers = start_shout_conns("radio.stew.moe".to_owned(), 8000, "source".to_owned(), "bDXsHJq9s2BvjWKeH5iO".to_owned(), radio_endpoints.clone());
    let mut handles = Vec::new();
    for (fmt, buffer) in buffers {
        match fmt {
            shout::ShoutFormat::Ogg => {
                let h = transcode::transcode(in_buf.clone(), ext.to_str().unwrap(), buffer.clone(), "ogg", ffmpeg::codec::id::Id::FLAC).unwrap();
                handles.push(h);
            }
            shout::ShoutFormat::MP3 => {
                let h = transcode::transcode(in_buf.clone(), ext.to_str().unwrap(), buffer.clone(), "mp3", ffmpeg::codec::id::Id::MP3).unwrap();
                handles.push(h);
            }
            _ => {
            
            }
        }
    }

    loop {
        thread::sleep(std::time::Duration::new(1, 0));
    }
}

fn start_shout_conns(host: String, port: u16, user: String, password: String, endpoints: Vec<(String, shout::ShoutFormat)>) -> Vec<(shout::ShoutFormat, Arc<Mutex<Vec<u8>>>)> {
    endpoints.into_iter().map(|(mount, format)| {
        let out_buf = Arc::new(Mutex::new(Vec::new()));
        let bc = out_buf.clone();
        // ://///
        let fc = format.clone();
        let hc = host.clone();
        let uc = user.clone();
        let pc = password.clone();
        thread::spawn(move || {
            let conn = shout::ShoutConnBuilder::new()
                .host(hc)
                .port(port)
                .user(uc)
                .password(pc)
                .mount(mount)
                .protocol(shout::ShoutProtocol::HTTP)
                .format(fc)
                .build()
                .unwrap();
            radio::play(conn, bc);
        });
        (format, out_buf)
    }).collect()
}
