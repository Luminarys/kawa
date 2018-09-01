use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{TcpStream, TcpListener, Ipv4Addr};
use std::{time, thread, cmp};
use std::io::{self, Read, Write};

use {amy, httparse};
use url::Url;

use api;
use config::{Config, StreamConfig, Container};

const CLIENT_BUFFER_LEN: usize = 16384;
// Number of frames to buffer by
const BACK_BUFFER_LEN: usize = 256;
// Seconds of inactivity until client timeout
const CLIENT_TIMEOUT: u64 = 10;

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
    listeners: api::Listeners,
    lid: usize,
    tid: usize,
    name: String,
}

#[derive(Clone, Debug)]
pub struct Buffer {
    mount: usize,
    data: BufferData
}

#[derive(Clone, Debug)]
pub enum BufferData {
    Header(Vec<u8>),
    Frame { data: Vec<u8>, pts: f64 },
    Trailer(Vec<u8>),
}

struct Client {
    conn: TcpStream,
    buffer: VecDeque<u8>,
    last_action: time::Instant,
    agent: Agent,
    chunker: Chunker,
}

#[derive(PartialEq)]
enum Agent {
    MPV,
    MPD,
    Other,
}

struct Incoming {
    last_action: time::Instant,
    conn: TcpStream,
    buf: [u8; 1024],
    len: usize,
}

struct Stream {
    config: StreamConfig,
    header: Vec<u8>,
    buffer: VecDeque<Vec<u8>>,
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

pub fn start(cfg: &Config, listeners: api::Listeners) -> amy::Sender<Buffer> {
    let (mut b, tx) = Broadcaster::new(cfg, listeners).unwrap();
    thread::spawn(move || b.run());
    tx
}

impl Broadcaster {
    pub fn new(cfg: &Config, listeners: api::Listeners) -> io::Result<(Broadcaster, amy::Sender<Buffer>)> {
        let poll = amy::Poller::new()?;
        let mut reg = poll.get_registrar()?;
        let listener = TcpListener::bind((Ipv4Addr::new(0, 0, 0, 0), cfg.radio.port))?;
        listener.set_nonblocking(true)?;
        let lid = reg.register(&listener, amy::Event::Read)?;
        let tid = reg.set_interval(5000)?;
        let (tx, rx) = reg.channel()?;
        let mut streams = Vec::new();
        for config in cfg.streams.iter().cloned() {
            streams.push(Stream { config, header: Vec::new(), buffer: VecDeque::with_capacity(BACK_BUFFER_LEN) })
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
            listeners,
            lid,
            tid,
            name: cfg.radio.name.clone(),
        }, tx))
    }

    pub fn run(&mut self) {
        debug!("starting broadcaster");
        loop {
            for n in self.poll.wait(15).unwrap() {
                if n.id == self.lid {
                    self.accept_client();
                } else if n.id == self.tid{
                    self.reap();
                } else if n.id == self.data.get_id() {
                    self.process_buffer();
                } else if self.incoming.contains_key(&n.id) {
                    self.process_incoming(n.id);
                } else if self.clients.contains_key(&n.id) {
                    self.process_client(n.id);
                } else {
                    warn!("Received amy event for bad id: {}", n.id);
                }
            }
        }
    }

    fn reap(&mut self) {
        let mut ids = Vec::new();
        for (id, inc) in self.incoming.iter() {
            if inc.last_action.elapsed() > time::Duration::from_secs(CLIENT_TIMEOUT) {
                ids.push(*id);
            }
        }
        for id in ids.iter() {
            self.remove_incoming(id);
        }
        ids.clear();

        for (id, client) in self.clients.iter() {
            if client.last_action.elapsed() > time::Duration::from_secs(CLIENT_TIMEOUT) {
                ids.push(*id);
            }
        }
        for id in ids.iter() {
            self.remove_client(id);
        }
    }

    fn accept_client(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((conn, ip)) => {
                    debug!("Accepted new connection from {:?}!", ip);
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
                    if buf.data.is_data() || client.agent == Agent::MPD {
                        client.send_data(buf.data.frame())
                    } else {
                        Ok(())
                    }
                }.is_err() {
                    self.remove_client(&id);
                }
            }
            {
                let ref mut sb = self.streams[buf.mount].buffer;
                sb.push_back(buf.data.frame().to_vec());
                while sb.len() > BACK_BUFFER_LEN {
                    sb.pop_front();
                }
            }
            match buf.data {
                BufferData::Header(h) => {
                    self.streams[buf.mount].header = h;
                }
                _ => { }
            }
        }
    }

    fn process_incoming(&mut self, id: usize) {
        match self.incoming.get_mut(&id).unwrap().process() {
            Ok(Some((path, agent, headers))) => {
                // Need this
                let ub = Url::parse("http://localhost/").unwrap();
                let url = if let Ok(u) = ub.join(&path) {
                    u
                } else {
                    self.remove_incoming(&id);
                    return;
                };
                let mount = url.path();

                let inc = self.incoming.remove(&id).unwrap();
                for (mid, stream) in self.streams.iter().enumerate() {
                    if mount.ends_with(&stream.config.mount) {
                        debug!("Adding a client to stream {}", stream.config.mount);
                        // Swap to write only mode
                        self.reg.reregister(id, &inc.conn, amy::Event::Write).unwrap();
                        let mut client = Client::new(inc.conn, agent);
                        // Send header, and buffered data
                        if client.write_resp(&self.name, &stream.config)
                            .and_then(|_| client.send_data(&stream.header))
                            .and_then(|_| {
                                // TODO: Consider edge case where the header is double sent
                                for buf in stream.buffer.iter() {
                                    client.send_data(buf)?
                                }
                                Ok(())
                            })
                            .is_ok()
                        {
                            self.client_mounts[mid].insert(id);
                            self.clients.insert(id, client);
                            self.listeners.lock().unwrap().insert(id, api::Listener {
                                mount: stream.config.mount.clone(),
                                path: path.clone(),
                                headers,
                            });
                        } else {
                            debug!("Failed to write data to client");
                        }
                        return;
                    }
                }
                debug!("Client specified unknown path: {}", mount);
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
        self.listeners.lock().unwrap().remove(id);
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
    pub fn new(mount: usize, data: BufferData) -> Buffer {
        Buffer { mount, data }
    }
}

impl BufferData {
    pub fn is_data(&self) -> bool {
        match *self {
            BufferData::Frame { .. } => true,
            _ => false,
        }
    }

    pub fn frame(&self) -> &[u8] {
        match *self {
            BufferData::Header(ref f)
            | BufferData::Frame { data: ref f, .. }
            | BufferData::Trailer(ref f) => f,
        }
    }
}

impl Incoming {
    fn new(conn: TcpStream) -> Incoming {
        conn.set_nonblocking(true).unwrap();
        Incoming {
            last_action: time::Instant::now(),
            conn,
            buf: [0; 1024],
            len: 0,
        }
    }

    fn process(&mut self) -> Result<Option<(String, Agent, Vec<api::Header>)>, ()> {
        self.last_action = time::Instant::now();
        if self.read().is_ok() {
            let mut headers = [httparse::EMPTY_HEADER; 16];
            let mut req = httparse::Request::new(&mut headers);
            match req.parse(&self.buf[..self.len]) {
                Ok(httparse::Status::Complete(_)) => {
                    if let Some(p) = req.path {
                        let mut agent = Agent::Other;
                        let ah: Vec<api::Header> = req.headers.into_iter()
                            .filter(|h| *h != &httparse::EMPTY_HEADER)
                            .map(|h| api::Header {
                                name: h.name.to_owned(),
                                value: String::from_utf8(h.value.to_vec()).unwrap_or("".to_owned())
                            })
                            .map(|h| {
                                if h.name == "User-Agent" {
                                    if h.value.starts_with("mpv") {
                                        agent = Agent::MPV;
                                    } else if h.value.starts_with("mpd") {
                                        agent = Agent::MPD;
                                    }
                                }
                                h
                            })
                            .collect();
                        Ok(Some((p.to_owned(), agent, ah)))
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
    fn new(conn: TcpStream, agent: Agent) -> Client {
        Client {
            conn,
            buffer: VecDeque::with_capacity(CLIENT_BUFFER_LEN),
            last_action: time::Instant::now(),
            chunker: Chunker::new(),
            agent,
        }
    }

    fn write_resp(&mut self, name: &str, config: &StreamConfig) -> Result<(), ()> {
        let lines = vec![
            format!("HTTP/1.1 200 OK"),
            format!("Server: {}/{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
            format!("Content-Type: {}", if let Container::MP3 = config.container {
                "audio/mpeg"
            } else if let Container::FLAC = config.container {
                "audio/flac"
            } else if let Container::AAC = config.container {
                "audio/aac"
            } else {
                "audio/ogg"
            }),
            format!("Transfer-Encoding: chunked"),
            format!("Connection: keep-alive"),
            format!("Cache-Control: no-cache"),
            format!("x-audiocast-name: {}", name),
            match config.bitrate {
		Some(bitrate) => format!("x-audiocast-bitrate: {}", bitrate),
		None => format!("x-audiocast-bitrate: 0"),
	    },
            format!("icy-name: {}", name),
            match config.bitrate {
                Some(bitrate) => format!("icy-br: {}", bitrate),
                None => format!("icy-br: 0"),
            },
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
        if data.len() == 0 {
            return Ok(());
        }

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
        self.last_action = time::Instant::now();

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

    fn write<T: io::Write>(&mut self, conn: &mut T, data: &[u8]) -> io::Result<Option<usize>> {
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
                    // Continue writing, and add on the current amount
                    // written to the result.
                    // We ignore errors here for now, since they should
                    // be reported later anyways. TODO: think more about it
                    match self.write(conn, &data[amnt..]) {
                        Ok(r) => {
                            Ok(Some(match r {
                                Some(a) => a + amnt,
                                None => amnt
                            }))
                        }
                        Err(_) => Ok(Some(amnt))
                    }
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
                    *self = Chunker::Footer(&s[amnt..]);
                    Ok(None)
                }
            }
        }
    }
}

#[test]
fn test_footer_transition() {
    use std::io::Cursor;
    let mut c = Chunker::Footer("hello world");
    let mut d = [0u8; 5];
    let mut v = Cursor::new(&mut d[..]);
    c.write(&mut v, &[]).unwrap();
    assert_eq!(v.into_inner(), b"hello");
    if let Chunker::Footer(" world") = c {
    } else { unreachable!() };
}
