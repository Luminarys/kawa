use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::io::Read;
use std::fs::File;
use std::path::Path;
use shout;

use Queue;
use config::{RadioConfig, StreamConfig};
use transcode;

pub fn start_streams(radio_cfg: RadioConfig,
                     stream_cfgs: Vec<StreamConfig>,
                     queue: Arc<Mutex<Queue>>) {
    let buffers: Vec<_> = stream_cfgs.iter()
        .map(|stream| {
            start_shout_conn(radio_cfg.host.clone(),
                             radio_cfg.port,
                             radio_cfg.user.clone(),
                             radio_cfg.password.clone(),
                             stream.mount.clone(),
                             stream.container.clone())
        })
        .collect();

    loop {
        let mut queue = queue.lock().unwrap();
        if let Some((_, path)) = queue.pop() {
            let mut f = File::open(&path).expect("invalid file");
            let mut in_buf = Vec::new();
            let ext = Path::new(&path).extension().unwrap();
            f.read_to_end(&mut in_buf).unwrap();
            let in_buf = Arc::new(in_buf);

            for (stream, buffer) in stream_cfgs.iter().zip(buffers.iter()) {
                match stream.container {
                    shout::ShoutFormat::Ogg => {
                        if let Err(e) = transcode::transcode(in_buf.clone(),
                                             ext.to_str().unwrap(),
                                             buffer.clone(),
                                             "ogg",
                                             stream.codec,
                                             stream.bitrate) {
                            panic!("{:?}", e)
                        }
                    }
                    shout::ShoutFormat::MP3 => {
                        transcode::transcode(in_buf.clone(),
                                             ext.to_str().unwrap(),
                                             buffer.clone(),
                                             "mp3",
                                             stream.codec,
                                             stream.bitrate)
                            .unwrap();
                    }
                    _ => {}
                };
            }
            loop {
                if buffers.iter().all(|buffer| buffer.lock().unwrap().len() == 0) {
                    break;
                } else {
                    thread::sleep(Duration::from_millis(100));
                }
            }
        } else {
            thread::sleep(Duration::from_millis(1000));
            // Autoq
        }
    }

    // let mut in_buf = Vec::new();
    // f.read_to_end(&mut in_buf).unwrap();
    // let in_buf = Arc::new(in_buf);

    // let radio_endpoints = vec![("/test.flac".to_owned(), shout::ShoutFormat::Ogg),
    //                            ("/test.mp3".to_owned(), shout::ShoutFormat::MP3)];
    // let buffers = start_shout_conns("radio.stew.moe".to_owned(),
    //                                 8000,
    //                                 "source".to_owned(),
    //                                 "bDXsHJq9s2BvjWKeH5iO".to_owned(),
    //                                 radio_endpoints.clone());
    // let mut handles = Vec::new();
    // for (fmt, buffer) in buffers {
    //     match fmt {
    //         shout::ShoutFormat::Ogg => {
    //             let h = transcode::transcode(in_buf.clone(),
    //                                          ext.to_str().unwrap(),
    //                                          buffer.clone(),
    //                                          "ogg",
    //                                          ffmpeg::codec::id::Id::FLAC)
    //                 .unwrap();
    //             handles.push(h);
    //         }
    //         shout::ShoutFormat::MP3 => {
    //             let h = transcode::transcode(in_buf.clone(),
    //                                          ext.to_str().unwrap(),
    //                                          buffer.clone(),
    //                                          "mp3",
    //                                          ffmpeg::codec::id::Id::MP3)
    //                 .unwrap();
    //             handles.push(h);
    //         }
    //         _ => {}
    //     }
    // }

    // loop {
    //     thread::sleep(Duration::new(1, 0));
    // }
}

pub fn play(conn: shout::ShoutConn, buffer: Arc<Mutex<Vec<u8>>>) {
    let step = 4096;
    loop {
        let mut data = buffer.lock().unwrap();
        if step < data.len() {
            conn.send(data.drain(0..step).collect());
            drop(data);
            conn.sync();
        } else if data.len() == 0 {
            thread::sleep(Duration::from_millis(100));
        } else {
            conn.send(data.drain(..).collect());
            conn.sync();
        }
    }
}

fn start_shout_conn(host: String,
                    port: u16,
                    user: String,
                    password: String,
                    mount: String,
                    format: shout::ShoutFormat)
                    -> Arc<Mutex<Vec<u8>>> {
    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let bc = out_buf.clone();
    thread::spawn(move || {
        let conn = shout::ShoutConnBuilder::new()
            .host(host)
            .port(port)
            .user(user)
            .password(password)
            .mount(mount)
            .protocol(shout::ShoutProtocol::HTTP)
            .format(format)
            .build()
            .unwrap();
        play(conn, bc);
    });
    out_buf
}
