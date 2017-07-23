use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{TcpStream, TcpListener};
use std::{time, thread, cmp};
use std::io::{self, Read, Write};

use {amy, httparse, shout};
use slog::Logger;

use config::{Config, StreamConfig};

const CLIENT_BUFFER_LEN: usize = 4096;
const CHUNK_SIZE: usize = 1024;
static CHUNK_HEADER: &'static str = "400\r\n";
static CHUNK_FOOTER: &'static str = "\r\n";

pub struct Broadcaster {
    poll: amy::Poller,
    reg: amy::Registrar,
    data: amy::Receiver<Buffer>,
    /// Map of amy ID -> incoming client
    incoming: HashMap<usize, Incoming>,
    /// Map from amy ID -> client
    clients: HashMap<usize, Client>,
    /// Vec of mount names, idx is mount id
    streams: Vec<Stream>,
    /// vec where idx: mount id , val: set of clients attached to mount id
    client_mounts: Vec<HashSet<usize>>,
    listener: TcpListener,
    lid: usize,
    log: Logger,
}

#[derive(Clone, Debug)]
pub struct Buffer {
    header: Option<Vec<u8>>,
    data: Vec<u8>,
    mount: usize
}

struct Client {
    conn: TcpStream,
    buffer: VecDeque<u8>,
    last_action: time::Instant,
    chunker: Chunker,
}

struct Incoming {
    conn: TcpStream,
    buf: [u8; 1024],
    len: usize,
}

struct Stream {
    config: StreamConfig,
    header: Vec<u8>,
}

enum Chunker {
    Header(&'static str),
    Body(usize),
    Footer(&'static str),
}

// Write
enum WR {
    Ok,
    Inc(usize),
    Blocked,
    Err,
}

pub fn start(cfg: &Config, log: Logger) -> amy::Sender<Buffer> {
    let (mut b, tx) = Broadcaster::new(cfg, log).unwrap();
    thread::spawn(move || b.run());
    tx
}

// TODO: Handle timeout
impl Broadcaster {
    pub fn new(cfg: &Config, log: Logger) -> io::Result<(Broadcaster, amy::Sender<Buffer>)> {
        let poll = amy::Poller::new()?;
        let mut reg = poll.get_registrar()?;
        let listener = TcpListener::bind("0.0.0.0:8001")?;
        listener.set_nonblocking(true)?;
        let lid = reg.register(&listener, amy::Event::Read)?;
        let (tx, rx) = reg.channel()?;
        let mut streams = Vec::new();
        for config in cfg.streams.iter().cloned() {
            streams.push(Stream { config, header: Vec::new(), })
        }

        Ok((Broadcaster {
            poll,
            reg,
            data: rx,
            incoming: HashMap::new(),
            clients: HashMap::new(),
            streams,
            client_mounts: vec![HashSet::new(); cfg.streams.len()],
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
        loop {
            match self.listener.accept() {
                Ok((conn, ip)) => {
                    debug!(self.log, "Accepted new connection from {:?}!", ip);
                    let pid = self.reg.register(&conn, amy::Event::Read).unwrap();
                    self.incoming.insert(pid, Incoming::new(conn));
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    break;
                }
                _ => { unimplemented!(); }
            }
        }

    }

    fn process_buffer(&mut self) {
        while let Ok(buf) = self.data.try_recv() {
            for id in self.client_mounts[buf.mount].clone() {
                if {
                    let client = self.clients.get_mut(&id).unwrap();
                    let r = if let Some(ref h) = buf.header {
                        client.send_data(h)
                    } else { Ok(()) };
                    r.and_then(|_| client.send_data(&buf.data[..]))
                }.is_err() {
                    self.remove_client(&id);
                }
            }
            if let Some(h) = buf.header {
                self.streams[buf.mount].header = h;
            }
        }
    }

    fn process_incoming(&mut self, id: usize) {
        match self.incoming.get_mut(&id).unwrap().process() {
            Ok(Some(mount)) => {
                let inc = self.incoming.remove(&id).unwrap();
                for (mid, stream) in self.streams.iter().enumerate() {
                    if mount.ends_with(&stream.config.mount) {
                        debug!(self.log, "Adding a client to stream {}", stream.config.mount);
                        // Swap to write only mode
                        self.reg.reregister(id, &inc.conn, amy::Event::Write).unwrap();
                        let mut client = Client::new(inc.conn);
                        if client.write_resp(&stream.config).and_then(|_| client.send_data(&stream.header)).is_ok() {
                            self.client_mounts[mid].insert(id);
                            self.clients.insert(id, client);
                        } else {
                            debug!(self.log, "Failed to write data to client");
                        }
                        return;
                    }
                }
                debug!(self.log, "Client specified unknown path: {}", mount);
            }
            Ok(None) => { },
            Err(()) => self.remove_incoming(&id),
        }
    }

    fn process_client(&mut self, id: usize) {
        match self.clients.get_mut(&id).unwrap().flush_buffer() {
            Err(()) => self.remove_client(&id),
            _ => { }
        }
    }

    fn remove_client(&mut self, id: &usize) {
        let client = self.clients.remove(id).unwrap();
        self.reg.deregister(&client.conn).unwrap();
        // Remove from client_mounts map too
        for m in self.client_mounts.iter_mut() {
            m.remove(id);
        }
    }

    fn remove_incoming(&mut self, id: &usize) {
        let inc = self.incoming.remove(id).unwrap();
        self.reg.deregister(&inc.conn).unwrap();
    }
}

impl Buffer {
    pub fn new(mount: usize, data: Vec<u8>) -> Buffer {
        Buffer { mount, data, header: None }
    }

    pub fn new_header(mount: usize, data: Vec<u8>, header: Vec<u8>) -> Buffer {
        Buffer { mount, data, header: Some(header) }
    }
}

impl Incoming {
    fn new(conn: TcpStream) -> Incoming {
        conn.set_nonblocking(true).unwrap();
        Incoming {
            conn,
            buf: [0; 1024],
            len: 0,
        }
    }

    fn process(&mut self) -> Result<Option<String>, ()> {
        if self.read().is_ok() {
            let mut headers = [httparse::EMPTY_HEADER; 16];
            let mut req = httparse::Request::new(&mut headers);
            match req.parse(&self.buf[..self.len]) {
                Ok(httparse::Status::Complete(_)) => {
                    if let Some(p) = req.path {
                        Ok(Some(p.to_owned()))
                    } else {
                        Err(())
                    }
                }
                Ok(httparse::Status::Partial) => Ok(None),
                Err(_) => Err(()),
            }
        } else {
            Err(())
        }
    }

    fn read(&mut self) -> Result<(), ()> {
        loop {
            match self.conn.read(&mut self.buf[self.len..]) {
                Ok(0) => return Err(()),
                Ok(a) => self.len += a,
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(_) => return Err(()),
            }
        }
    }
}

impl Client {
    fn new(conn: TcpStream) -> Client {
        Client {
            conn,
            buffer: VecDeque::with_capacity(CLIENT_BUFFER_LEN),
            last_action: time::Instant::now(),
            chunker: Chunker::new(),
        }
    }

    fn write_resp(&mut self, config: &StreamConfig) -> Result<(), ()> {
        let lines = vec![
            format!("HTTP/1.1 200 OK"),
            format!("Server: {}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
            format!("Content-Type: {}", if let shout::ShoutFormat::MP3 = config.container {
                "audio/mpeg"
            } else {
                "application/ogg"
            }),
            format!("Transfer-Encoding: chunked"),
            format!("Connection: keep-alive"),
            format!("Cache-Control: no-cache"),
        ];
        let data = lines.join("\r\n") + "\r\n\r\n";
        match self.conn.write(data.as_bytes()) {
            Ok(0) => Err(()),
            Ok(a) if a == data.as_bytes().len() => { Ok(() )}
            Ok(_) => unreachable!(),
            Err(_) => Err(())
        }
    }

    fn send_data(&mut self, data: &[u8]) -> Result<(), ()> {
        self.last_action = time::Instant::now();
        // Attempt to flush buffer first
        match self.flush_buffer() {
            Ok(true) => { },
            Ok(false) => {
                self.buffer.extend(data.iter());
                while self.buffer.len() > CLIENT_BUFFER_LEN {
                    self.buffer.pop_front();
                }
                return Ok(())
            },
            Err(()) => return Err(()),
        }

        match self.chunker.write(&mut self.conn, data) {
            Ok(Some(0)) => Err(()),
            // Complete write, do nothing
            Ok(Some(a)) if a == data.len() => Ok(()),
            // Incomplete write, append to buf
            Ok(Some(a)) => {
                self.buffer.extend(data[0..a].iter());
                while self.buffer.len() > CLIENT_BUFFER_LEN {
                    self.buffer.pop_front();
                }
                Ok(())
            }
            Ok(None) => {
                self.buffer.extend(data.iter());
                while self.buffer.len() > CLIENT_BUFFER_LEN {
                    self.buffer.pop_front();
                }
                Ok(())
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.buffer.extend(data.iter());
                while self.buffer.len() > CLIENT_BUFFER_LEN {
                    self.buffer.pop_front();
                }
                Ok(())
            }
            Err(_) => Err(()),
        }
    }

    fn flush_buffer(&mut self) -> Result<bool, ()> {
        if self.buffer.is_empty() {
            return Ok(true);
        }
        loop {
            match self.write_buffer() {
                WR::Ok => {
                    self.buffer.clear();
                    return Ok(true);
                }
                WR::Inc(a) => {
                    for _ in 0..a {
                        self.buffer.pop_front();
                    }
                }
                WR::Blocked => return Ok(false),
                WR::Err => return Err(()),
            }
        }
    }

    fn write_buffer(&mut self) -> WR {
        let (head, tail) = self.buffer.as_slices();
        match self.chunker.write(&mut self.conn, head) {
            Ok(Some(0)) => WR::Err,
            Ok(Some(a)) if a == head.len() => {
                match self.chunker.write(&mut self.conn, tail) {
                    Ok(Some(0)) => WR::Err,
                    Ok(Some(i)) if i == tail.len() => WR::Ok,
                    Ok(Some(i)) => WR::Inc(i + a),
                    Ok(None) => WR::Inc(a),
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => WR::Inc(a),
                    Err(_) => WR::Err,
                }
            },
            Ok(Some(a)) => WR::Inc(a),
            Ok(None) => WR::Inc(0),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => WR::Blocked,
            Err(_) => WR::Err,
        }
    }
}

impl Chunker {
    fn new() -> Chunker {
        Chunker::Header(CHUNK_HEADER)
    }

    fn write(&mut self, conn: &mut TcpStream, data: &[u8]) -> io::Result<Option<usize>> {
        match *self {
            Chunker::Header(s) => {
                let amnt = conn.write(s.as_bytes())?;
                if amnt == s.len() {
                    *self = Chunker::Body(0);
                    self.write(conn, data)
                } else {
                    *self = Chunker::Header(&s[amnt..]);
                    Ok(None)
                }
            }
            Chunker::Body(i) => {
                let amnt = conn.write(&data[..cmp::min(CHUNK_SIZE - i, data.len())])?;
                if i + amnt == CHUNK_SIZE {
                    *self = Chunker::Footer(CHUNK_FOOTER);
                    self.write(conn, &data[amnt..]).map(|r| r.map(|a| a + amnt))
                } else {
                    *self = Chunker::Body(i + amnt);
                    Ok(Some(amnt))
                }
            }
            Chunker::Footer(s) => {
                let amnt = conn.write(s.as_bytes())?;
                if amnt == s.len() {
                    *self = Chunker::Header(CHUNK_HEADER);
                    self.write(conn, data)
                } else {
                    *self = Chunker::Header(&s[amnt..]);
                    Ok(None)
                }
            }
        }
    }
}
