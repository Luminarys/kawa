use std::sync::{Arc, Mutex};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;
use std::io::Read;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::mem;

use shout;
use queue::{Queue, QueueEntry};
use api::ApiMessage;
use config::{RadioConfig, StreamConfig};
use transcode;
use ring_buffer::RingBuffer;

fn initiate_transcode(path: String,
                      stream_cfgs: &Vec<StreamConfig>)
                      -> Option<(Vec<Arc<RingBuffer<u8>>>, Vec<Arc<AtomicBool>>)> {
    let mut in_buf = Vec::new();
    let mut compl_tokens = Vec::new();
    let mut buffers = Vec::new();

    let ext = match Path::new(&path).extension() {
        Some(e) => e,
        None => return None,
    };

    if let None = File::open(&path).ok().and_then(|mut f| f.read_to_end(&mut in_buf).ok()) {
        return None;
    }
    let in_buf = Arc::new(in_buf);

    for stream in stream_cfgs.iter() {
        let token = Arc::new(AtomicBool::new(false));
        // 500KB Buffer
        let out_buf = Arc::new(RingBuffer::new(500000));
        match stream.container {
            shout::ShoutFormat::Ogg => {
                if let Err(e) = transcode::transcode(in_buf.clone(),
                                                     ext.to_str().unwrap(),
                                                     out_buf.clone(),
                                                     "ogg",
                                                     stream.codec,
                                                     stream.bitrate,
                                                     token.clone()) {
                    println!("WARNING: Transcoder creation failed with error: {:?}", e);
                } else {
                    buffers.push(out_buf.clone());
                    compl_tokens.push(token);
                }
            }
            shout::ShoutFormat::MP3 => {
                if let Err(e) = transcode::transcode(in_buf.clone(),
                                                     ext.to_str().unwrap(),
                                                     out_buf.clone(),
                                                     "mp3",
                                                     stream.codec,
                                                     stream.bitrate,
                                                     token.clone()) {
                    println!("WARNING: Transcoder creation failed with error: {:?}", e);
                } else {
                    buffers.push(out_buf.clone());
                    compl_tokens.push(token);
                }
            }
            _ => {}
        };
    }
    Some((buffers, compl_tokens))
}

fn get_queue_prebuf(queue: Arc<Mutex<Queue>>,
                    configs: &Vec<StreamConfig>)
                    -> Option<(Vec<Arc<RingBuffer<u8>>>, Vec<Arc<AtomicBool>>)> {
    let queue = queue.lock().unwrap();
    if queue.entries.is_empty() {
        return None;
    }
    return initiate_transcode(queue.entries[0].path.clone(), configs);
}

fn get_random_prebuf(configs: &Vec<StreamConfig>)
    -> (Vec<Arc<RingBuffer<u8>>>, Vec<Arc<AtomicBool>>) {
    let mut counter = 0;
    loop {
        if counter == 100 {
            panic!("Your random shit is broken.");
        }
        let random = get_random_song();
        if let Some(p) = initiate_transcode(random.path.clone(), configs) {
            return p;
        }
        counter += 1;
    }
}

pub fn start_streams(radio_cfg: RadioConfig,
                     stream_cfgs: Vec<StreamConfig>,
                     queue: Arc<Mutex<Queue>>,
                     updates: Receiver<ApiMessage>) {
    let mut random_prebuf = get_random_prebuf(&stream_cfgs);
    let mut queue_prebuf = get_queue_prebuf(queue.clone(), &stream_cfgs);
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
        let (buffers, tokens) = if queue_prebuf.is_some() {
            queue.lock().unwrap().entries.pop();
            mem::replace(&mut queue_prebuf, None).unwrap()
        } else {
            mem::replace(&mut random_prebuf, get_random_prebuf(&stream_cfgs))
        };

        for (buffer, chan) in buffers.iter().zip(buf_chans.iter()) {
            chan.send(buffer.clone()).unwrap();
        }

        loop {
            if queue_prebuf.is_none() {
                queue_prebuf = get_queue_prebuf(queue.clone(), &stream_cfgs);
            }

            if tokens.iter()
                     .zip(buffers.iter())
                     .all(|(token, buffer)| token.load(Ordering::SeqCst) && buffer.len() == 0) {
                break;
            } else {
                if let Ok(msg) = updates.try_recv() {
                    match msg {
                        ApiMessage::Skip => {
                            println!("Skipping!");
                            for token in tokens.iter() {
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
    }
}

fn get_random_song() -> QueueEntry {
    QueueEntry::new("".to_owned(), "/tmp/in.flac".to_owned())
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
