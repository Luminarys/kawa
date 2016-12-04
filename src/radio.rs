use std::sync::{Arc, Mutex};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;
use std::sync::atomic::Ordering;

use shout;
use queue::Queue;
use api::{ApiMessage, QueuePos};
use config::Config;
use ring_buffer::RingBuffer;

struct RadioConn {
    tx: Sender<Arc<RingBuffer<u8>>>,
    handle: thread::JoinHandle<()>,
}

impl RadioConn {
    fn new(host: String,
           port: u16,
           user: String,
           password: String,
           mount: String,
           format: shout::ShoutFormat) -> RadioConn {
        let (tx, rx) = mpsc::channel();

        let handle = thread::spawn(move || {
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
        RadioConn {
            tx: tx,
            handle: handle,
        }
    }

    fn replace_buffer(&mut self, buffer: Arc<RingBuffer<u8>>) {
        self.tx.send(buffer).unwrap();
    }
}

pub fn play(conn: shout::ShoutConn, buffer_rec: Receiver<Arc<RingBuffer<u8>>>) {
    let step = 4096;
    let mut buffer = buffer_rec.recv().unwrap();
    loop {
        match buffer_rec.try_recv() {
            Ok(b) => { buffer = b; }
            Err(TryRecvError::Empty) => { }
            Err(TryRecvError::Disconnected) => { return; }
        }

        if buffer.len() > 0 {
            if let Err(_) = conn.send(buffer.try_read(step)) {
                if let Err(_) = conn.reconnect() {
                    panic!("Reconn failure, shutting down!");
                }
            }
            conn.sync();
        } else {
            // We're starved, probably should not happen
            thread::sleep(Duration::from_millis(100));
        }
    }
}

pub fn start_streams(cfg: Config,
                     queue: Arc<Mutex<Queue>>,
                     updates: Receiver<ApiMessage>) {
    let mut rconns: Vec<_> = cfg.streams.iter()
        .map(|stream| {
            RadioConn::new(cfg.radio.host.clone(),
                             cfg.radio.port,
                             cfg.radio.user.clone(),
                             cfg.radio.password.clone(),
                             stream.mount.clone(),
                             stream.container.clone())
        })
        .collect();

    queue.lock().unwrap().start_next_tc();
    loop {
        let prebuffers = queue.lock().unwrap().get_next_tc();

        for (rconn, pb) in rconns.iter_mut().zip(prebuffers.iter()) {
            rconn.replace_buffer(pb.buffer.clone());
        }
        queue.lock().unwrap().pop_head();

        // Song activity loop - ensures that the song is properly transcoding and handles any sort
        // of API message that gets received in the meanwhile
        loop {
            // If the prebuffers are all completed, complete loop iteration, requeue next song
            if prebuffers.iter()
                .all(|prebuffer| {
                    prebuffer.token.load(Ordering::Acquire) && prebuffer.buffer.len() == 0
                }) {
                break;
            } else {
                if let Ok(msg) = updates.try_recv() {
                    // Keep all these operations local just incase
                    // anything complex might need to happen in the future.
                    match msg {
                        ApiMessage::Skip => {
                            break;
                        }
                        ApiMessage::Clear => {
                            queue.lock().unwrap().clear();
                        }
                        ApiMessage::Insert(QueuePos::Head, qe) => {
                            queue.lock().unwrap().push_head(qe);
                        }
                        ApiMessage::Insert(QueuePos::Tail, qe) => {
                            queue.lock().unwrap().push(qe);
                        }
                        ApiMessage::Remove(QueuePos::Head) => {
                            queue.lock().unwrap().pop_head();
                        }
                        ApiMessage::Remove(QueuePos::Tail) => {
                            queue.lock().unwrap().pop();
                        }
                    }
                } else {
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
}
