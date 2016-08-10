use std::sync::{Arc, Mutex};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;
use std::io::Read;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use shout;
use queue::{Queue, QueueEntry};
use api::ApiMessage;
use config::{RadioConfig, StreamConfig};
use transcode;
use ring_buffer::RingBuffer;

pub fn start_streams(radio_cfg: RadioConfig,
                     stream_cfgs: Vec<StreamConfig>,
                     queue: Arc<Mutex<Queue>>,
                     updates: Receiver<ApiMessage>) {
    let buf_chans: Vec<_> = stream_cfgs.iter()
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
        let mut compl_tokens = Vec::new();
        let mut buffers = Vec::new();
        if let Some(ref qe) = queue.entries.pop() {
            let ref path = qe.path;
            let mut f = match File::open(&path) {
                Ok(f) => f,
                Err(_) => break,
            };
            let mut in_buf = Vec::new();
            let ext = Path::new(&path).extension().unwrap();
            f.read_to_end(&mut in_buf).unwrap();
            let in_buf = Arc::new(in_buf);

            for (stream, chan) in stream_cfgs.iter().zip(buf_chans.iter()) {
                let token = Arc::new(AtomicBool::new(false));
                // 500KB Buffer
                let out_buf = Arc::new(RingBuffer::new(500000));
                match stream.container {
                    shout::ShoutFormat::Ogg => {
                        transcode::transcode(in_buf.clone(),
                                             ext.to_str().unwrap(),
                                             out_buf.clone(),
                                             "ogg",
                                             stream.codec,
                                             stream.bitrate,
                                             token.clone())
                            .unwrap();
                        buffers.push(out_buf.clone());
                        chan.send(out_buf).unwrap();
                        compl_tokens.push(token);
                    }
                    shout::ShoutFormat::MP3 => {
                        transcode::transcode(in_buf.clone(),
                                             ext.to_str().unwrap(),
                                             out_buf.clone(),
                                             "mp3",
                                             stream.codec,
                                             stream.bitrate,
                                             token.clone())
                            .unwrap();
                        buffers.push(out_buf.clone());
                        chan.send(out_buf).unwrap();
                        compl_tokens.push(token);
                    }
                    _ => {}
                };
            }
            drop(queue);
            loop {
                if compl_tokens.iter().zip(buffers.iter()).all(|(token, buffer)| {
                    token.load(Ordering::SeqCst) && buffer.len() == 0
                }) {
                    break;
                } else {
                    if let Ok(msg) = updates.try_recv() {
                        match msg {
                            ApiMessage::Skip => {
                                println!("Skipping!");
                                for token in compl_tokens.iter() {
                                    token.store(true, Ordering::SeqCst);
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
            println!("Autoqueing!");
            queue.entries.push(QueueEntry::new("".to_owned(), "/tmp/in.flac".to_owned()));
            drop(queue);
            thread::sleep(Duration::from_millis(1000));
        }
    }
}

pub fn play(conn: shout::ShoutConn, buffer_rec: Receiver<Arc<RingBuffer<u8>>>) {
    let step = 4096;
    let mut buffer = buffer_rec.recv().unwrap();
    loop {
        if let Ok(b) = buffer_rec.try_recv() {
            buffer = b;
        }

        if buffer.len() > 0 {
            conn.send(buffer.try_read(step));
            conn.sync();
        } else {
            thread::sleep(Duration::from_millis(100));
        }
    }
}

fn start_shout_conn(host: String,
                    port: u16,
                    user: String,
                    password: String,
                    mount: String,
                    format: shout::ShoutFormat)
                    -> Sender<Arc<RingBuffer<u8>>> {
    let (tx, rx) = mpsc::channel();

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
        play(conn, rx);
    });
    tx
}
