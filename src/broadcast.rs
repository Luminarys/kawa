use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{TcpStream, TcpListener};
use std::{time, io};

use {amy, httparse};
use slog::Logger;

use config::Config;

pub struct Broadcaster {
    poll: amy::Poller,
    reg: amy::Registrar,
    data: amy::Receiver<Buffer>,
    /// Map of amy ID -> incoming client
    incoming: HashMap<usize, Client>,
    /// Map from amy ID -> client
    clients: HashMap<usize, Client>,
    /// Map from mount name(e.g. stream256.opus) -> mount id
    mounts: HashMap<String, usize>,
    /// vec where idx: mount id , val: set of clients attached to mount id
    clientMounts: Vec<HashSet<usize>>,
    listener: TcpListener,
    lid: usize,
    log: Logger,
}

#[derive(Clone, Debug)]
pub struct Buffer {
    data: Vec<u8>,
    mount: usize
}

struct Client {
    conn: TcpStream,
    buffer: VecDeque<u8>,
    last_action: time::Instant,
}

impl Broadcaster {
    pub fn new(cfg: &Config, log: Logger) -> io::Result<(Broadcaster, amy::Sender<Buffer>)> {
        let poll = amy::Poller::new()?;
        let mut reg = poll.get_registrar()?;
        let listener = TcpListener::bind("0.0.0.0:8001")?;
        listener.set_nonblocking(true)?;
        let lid = reg.register(&listener, amy::Event::Read)?;
        let (tx, rx) = reg.channel()?;
        let mut mounts = HashMap::new();
        let mut clientMounts = Vec::new();
        for (id, stream) in cfg.streams.iter().enumerate() {
            mounts.insert(stream.mount.clone(), id);
            clientMounts.push(HashSet::new());
        }

        Ok((Broadcaster {
            poll,
            reg,
            data: rx,
            incoming: HashMap::new(),
            clients: HashMap::new(),
            mounts,
            clientMounts,
            listener,
            lid,
            log,
        }, tx))
    }

    pub fn run(&mut self) {
        debug!(self.log, "starting broadcaster");
        loop {
            for n in self.poll.wait(15).unwrap() {
                if n.id == self.lid {
                    self.accept_client();
                } else if n.id == self.data.get_id() {
                    self.process_buffer();
                } else if self.incoming.contains_key(&n.id) {
                    self.process_incoming(n.id);
                } else if self.clients.contains_key(&n.id) {
                    self.process_client(n.id);
                } else {
                    warn!(self.log, "Received amy event for bad id: {}", n.id);
                }
            }
        }
    }

    fn accept_client(&mut self) {
    }

    fn process_buffer(&mut self) {
    }

    fn process_incoming(&mut self, id: usize) {
    }

    fn process_client(&mut self, id: usize) {
    }
}
