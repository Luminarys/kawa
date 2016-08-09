use std::sync::{Arc, Mutex};
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::Duration;
use std::io::Read;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use shout;
use queue::Queue;
use api::ApiMessage;
use config::{RadioConfig, StreamConfig};
use transcode;

pub fn start_streams(radio_cfg: RadioConfig,
                     stream_cfgs: Vec<StreamConfig>,
                     queue: Arc<Mutex<Queue>>,
                     updates: Receiver<ApiMessage>) {
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
        let mut cancel_tokens = Vec::new();
        if let Some(ref qe) = queue.entries.pop() {
            let ref path = qe.path;
            let mut f = File::open(&path).expect("invalid file");
            let mut in_buf = Vec::new();
            let ext = Path::new(&path).extension().unwrap();
            f.read_to_end(&mut in_buf).unwrap();
            let in_buf = Arc::new(in_buf);

            for (stream, buffer) in stream_cfgs.iter().zip(buffers.iter()) {
                let token = Arc::new(AtomicBool::new(false));
                match stream.container {
                    shout::ShoutFormat::Ogg => {
                        transcode::transcode(in_buf.clone(),
                                             ext.to_str().unwrap(),
                                             buffer.clone(),
                                             "ogg",
                                             stream.codec,
                                             stream.bitrate,
                                             token.clone())
                            .unwrap();
                        cancel_tokens.push(token);
                    }
                    shout::ShoutFormat::MP3 => {
                        transcode::transcode(in_buf.clone(),
                                             ext.to_str().unwrap(),
                                             buffer.clone(),
                                             "mp3",
                                             stream.codec,
                                             stream.bitrate,
                                             token.clone())
                            .unwrap();
                        cancel_tokens.push(token);
                    }
                    _ => {}
                };
            }
            drop(queue);
            loop {
                if buffers.iter().all(|buffer| buffer.lock().unwrap().len() == 0) {
                    println!("All buffers flushed!");
                    break;
                } else {
                    if let Ok(msg) = updates.try_recv() {
                        println!("Received chan message!");
                        match msg {
                            ApiMessage::Skip => {
                                println!("Skipping!");
                                for token in cancel_tokens.iter() {
                                    token.store(true, Ordering::SeqCst);
                                    assert!(token.load(Ordering::SeqCst), true);
                                }
                                break;
                            }
                        }
                    } else {
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        } else {
            drop(queue);
            println!("Autoqueing!");
            thread::sleep(Duration::from_millis(1000));
        }
    }
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
