use std::sync::{Arc, Mutex};
use std::sync::mpsc::Sender;
use std::thread;
use std::path::Path;
use serde_json as serde;
use slog::Logger;
use rouille;

use queue::{Queue, QueueEntry};
use config::ApiConfig;

#[derive(Debug)]
pub enum QueuePos {
    Head,
    Tail,
}

#[derive(Debug)]
pub enum ApiMessage {
    Skip,
    Remove(QueuePos),
    Insert(QueuePos, QueueEntry),
    Clear,
}

type SQueue = Arc<Mutex<Queue>>;
type ApiChan = Arc<Mutex<Sender<ApiMessage>>>;

#[derive(Serialize)]
pub struct Resp {
    pub success: bool,
    pub reason: Option<String>,
}

impl Resp {
    fn success() -> Resp {
        Resp {
            success: true,
            reason: None,
        }
    }

    fn failure(reason: &str) -> Resp {
        Resp {
            success: false,
            reason: Some(String::from(reason)),
        }

    }
}

struct Server {
    queue: SQueue,
    chan: ApiChan,
    log: Logger,
}

impl Server {
    fn handle_request(&self, req: &rouille::Request) -> rouille::Response {
        router!(req,
                (GET) (/np) => {
                    debug!(self.log, "Handling now playing req");
                    let q = self.queue.lock().unwrap();
                    rouille::Response::from_data(
                        "application/json",
                        serde::to_string(q.np().entry()).unwrap())
                },

                (GET) (/queue) => {
                    debug!(self.log, "Handling queue disp req");
                    let q = self.queue.lock().unwrap();
                    rouille::Response::from_data(
                        "application/json",
                        serde::to_string(&q.entries()).unwrap())
                },

                (POST) (/queue/head) => {
                    match serde::from_reader(req.data().unwrap()) {
                        Ok(qe) => {
                            debug!(self.log, "Handling queue head insert");
                            let qe: QueueEntry = qe;
                            if Path::new(&qe.path).exists() {
                                self.chan.lock().unwrap().send(ApiMessage::Insert(QueuePos::Head, qe)).unwrap();
                                rouille::Response::from_data(
                                    "application/json",
                                    serde::to_string(&Resp::success()).unwrap())
                            } else {
                                rouille::Response::from_data(
                                    "application/json",
                                    serde::to_string(&Resp::failure("file does not exist")).unwrap()
                                ).with_status_code(400)
                            }
                        }
                        Err(_) => {
                            rouille::Response::from_data(
                                "application/json",
                                serde::to_string(&Resp::failure("malformed body")).unwrap()
                            ).with_status_code(400)
                        }
                    }
                },

                (DELETE) (/queue/head) => {
                    debug!(self.log, "Handling queue head remove");
                    self.chan.lock().unwrap().send(ApiMessage::Remove(QueuePos::Head)).unwrap();
                    rouille::Response::from_data(
                        "application/json",
                        serde::to_string(&Resp::success()).unwrap())
                },

                (POST) (/queue/tail) => {
                    debug!(self.log, "Handling queue tail insert");
                    match serde::from_reader(req.data().unwrap()) {
                        Ok(qe) => {
                            let qe: QueueEntry = qe;
                            if Path::new(&qe.path).exists() {
                                self.chan.lock().unwrap().send(ApiMessage::Insert(QueuePos::Tail, qe)).unwrap();
                                rouille::Response::from_data(
                                    "application/json",
                                    serde::to_string(&Resp::success()).unwrap())
                            } else {
                                rouille::Response::from_data(
                                    "application/json",
                                    serde::to_string(&Resp::failure("file does not exist")).unwrap()
                                ).with_status_code(400)
                            }
                        }
                        Err(_) => {
                            rouille::Response::from_data(
                                "application/json",
                                serde::to_string(&Resp::failure("malformed body")).unwrap()
                            ).with_status_code(400)
                        }
                    }
                },

                (DELETE) (/queue/tail) => {
                    debug!(self.log, "Handling queue tail remove");
                    self.chan.lock().unwrap().send(ApiMessage::Remove(QueuePos::Tail)).unwrap();
                    rouille::Response::from_data(
                        "application/json",
                        serde::to_string(&Resp::success()).unwrap())
                },

                (POST) (/queue/skip) => {
                    debug!(self.log, "Handling queue skip");
                    self.chan.lock().unwrap().send(ApiMessage::Skip).unwrap();
                    rouille::Response::from_data(
                        "application/json",
                        serde::to_string(&Resp::success()).unwrap())
                },

                (POST) (/queue/clear) => {
                    debug!(self.log, "Handling queue clear");
                    self.chan.lock().unwrap().send(ApiMessage::Clear).unwrap();
                    rouille::Response::from_data(
                        "application/json",
                        serde::to_string(&Resp::success()).unwrap())
                },

                _ => rouille::Response::empty_404()
            )
    }
}

pub fn start_api(config: ApiConfig, queue: Arc<Mutex<Queue>>, updates: Sender<ApiMessage>, log: Logger) {
    thread::spawn(move || {
        info!(log, "Starting API");
        let chan = Arc::new(Mutex::new(updates));
        let serv = Server {
            queue: queue,
            chan: chan,
            log: log,
        };
        rouille::start_server(("127.0.0.1", config.port), move |request| {
            serv.handle_request(request)
        });
    });
}
