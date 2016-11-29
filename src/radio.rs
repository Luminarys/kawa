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
use api::{ApiMessage, QueuePos};
use config::{RadioConfig, StreamConfig};
use transcode;
use ring_buffer::RingBuffer;

struct PreBuffer {
    buffer: Arc<RingBuffer<u8>>,
    token: Arc<AtomicBool>,
}

impl PreBuffer {
    fn cancel(self) {
        self.token.store(true, Ordering::SeqCst);
        loop {
            if self.buffer.len() > 0 {
                self.buffer.try_read(4096);
            } else {
                break;
            }
        }
    }
}

fn initiate_transcode(path: String, stream_cfgs: &Vec<StreamConfig>) -> Option<Vec<PreBuffer>> {
    let mut in_buf = Vec::new();
    let mut prebufs = Vec::new();

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
                    prebufs.push(PreBuffer {
                        buffer: out_buf.clone(),
                        token: token,
                    });
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
                    prebufs.push(PreBuffer {
                        buffer: out_buf.clone(),
                        token: token,
                    });
                }
            }
            _ => {}
        };
    }
    Some(prebufs)
}

fn get_queue_prebuf(queue: Arc<Mutex<Queue>>,
                    configs: &Vec<StreamConfig>)
                    -> Option<Vec<PreBuffer>> {
    let mut queue = queue.lock().unwrap();
    while !queue.entries.is_empty() {
        if let Some(r) = initiate_transcode(queue.entries[0].path.clone(), configs) {
            return Some(r);
        } else {
            queue.entries.pop();
        }
    }
    None
}

fn get_random_prebuf(configs: &Vec<StreamConfig>) -> Vec<PreBuffer> {
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
        let prebuffers = if queue_prebuf.is_some() {
            queue.lock().unwrap().entries.remove(0);
            mem::replace(&mut queue_prebuf,
                         get_queue_prebuf(queue.clone(), &stream_cfgs))
                .unwrap()
        } else {
            mem::replace(&mut random_prebuf, get_random_prebuf(&stream_cfgs))
        };

        for (prebuffer, chan) in prebuffers.iter().zip(buf_chans.iter()) {
            chan.send(prebuffer.buffer.clone()).unwrap();
        }

        loop {
            if prebuffers.iter()
                .all(|prebuffer| {
                    prebuffer.token.load(Ordering::SeqCst) && prebuffer.buffer.len() == 0
                }) {
                break;
            } else {
                if let Ok(msg) = updates.try_recv() {
                    match msg {
                        ApiMessage::Skip => {
                            for prebuffer in prebuffers.iter() {
                                prebuffer.token.store(true, Ordering::SeqCst);
                            }
                            break;
                        }
                        ApiMessage::Clear => {
                            if queue_prebuf.is_some() {
                                for prebuf in mem::replace(&mut queue_prebuf, None).unwrap() {
                                    prebuf.cancel();
                                }
                            }
                            queue.lock().unwrap().clear();
                        }
                        ApiMessage::Insert(QueuePos::Head, qe) => {
                            {
                                let mut q = queue.lock().unwrap();
                                q.insert(0, qe);
                            }
                            let old_prebufs = mem::replace(&mut queue_prebuf,
                                                           get_queue_prebuf(queue.clone(),
                                                                            &stream_cfgs))
                                .unwrap();
                            for prebuf in old_prebufs {
                                prebuf.cancel();
                            }
                        }
                        ApiMessage::Insert(QueuePos::Tail, qe) => {
                            let mut q = queue.lock().unwrap();
                            q.push(qe);
                            if q.len() == 1 {
                                drop(q);
                                queue_prebuf = get_queue_prebuf(queue.clone(), &stream_cfgs);
                            }
                        }
                        ApiMessage::Remove(QueuePos::Head) => {
                            let mut q = queue.lock().unwrap();
                            if q.len() > 0 {
                                q.remove(0);
                                drop(q);
                                let old_prebufs = mem::replace(&mut queue_prebuf,
                                                               get_queue_prebuf(queue.clone(),
                                                                                &stream_cfgs))
                                    .unwrap();
                                for prebuf in old_prebufs {
                                    prebuf.cancel();
                                }
                            }
                        }
                        ApiMessage::Remove(QueuePos::Tail) => {
                            let mut q = queue.lock().unwrap();
                            if q.len() > 0 {
                                q.pop();
                            }
                            if q.len() == 0 {
                                drop(q);
                                if let Some(old_prebufs) = mem::replace(&mut queue_prebuf, None) {
                                    for prebuf in old_prebufs {
                                        prebuf.cancel();
                                    }
                                }
                            }
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
