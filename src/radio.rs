use std::sync::{Arc, Mutex};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;
use std::sync::atomic::Ordering;
use slog::Logger;

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
           format: shout::ShoutFormat,
           log: Logger) -> RadioConn {
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
            play(conn, rx, log);
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

pub fn play(conn: shout::ShoutConn, buffer_rec: Receiver<Arc<RingBuffer<u8>>>, log: Logger) {
    let step = 4096;
    debug!(log, "Awaiting initial buffer");
    let mut buffer = buffer_rec.recv().unwrap();
    loop {
        match buffer_rec.try_recv() {
            Ok(b) => {
                debug!(log, "Received new buffer");
                buffer = b;
            }
            Err(TryRecvError::Empty) => { }
            Err(TryRecvError::Disconnected) => { return; }
        }

        if buffer.len() > 0 {
            if let Err(_) = conn.send(buffer.try_read(step)) {
                warn!(log, "Failed to send data, attempting to reconnect");
                if let Err(_) = conn.reconnect() {
                    crit!(log, "Failed to reconnect");
                }
            }
            conn.sync();
        } else {
            // We're starved, probably should not happen
            warn!(log, "Starved for data!");
            thread::sleep(Duration::from_millis(100));
        }
    }
}

pub fn start_streams(cfg: Config,
                     queue: Arc<Mutex<Queue>>,
                     updates: Receiver<ApiMessage>,
                     log: Logger) {
    let mut rconns: Vec<_> = cfg.streams.iter()
        .map(|stream| {
            let rlog = log.new(o!("mount" => stream.mount.clone()));
            RadioConn::new(cfg.radio.host.clone(),
                             cfg.radio.port,
                             cfg.radio.user.clone(),
                             cfg.radio.password.clone(),
                             stream.mount.clone(),
                             stream.container.clone(),
                             rlog)
        })
        .collect();

    debug!(log, "Obtaining initial transcode buffer");
    queue.lock().unwrap().start_next_tc();
    thread::sleep(Duration::from_millis(100));
    loop {
        debug!(log, "Extracting next buffer");
        let prebuffers = queue.lock().unwrap().get_next_tc();

        debug!(log, "Dispatching new buffers");
        for (rconn, pb) in rconns.iter_mut().zip(prebuffers.iter()) {
            // The order is guarenteed to be correct because we always iterate by the config
            // ordering.
            rconn.replace_buffer(pb.buffer.clone());
        }
        debug!(log, "Removing queue head");
        queue.lock().unwrap().pop_head();
        debug!(log, "Entering main loop");

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
                    debug!(log, "Received API message {:?}", msg);
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
