use std::sync::{Arc, Mutex};
use std::sync::mpsc::Sender;
use std::collections::HashMap;
use std::thread;
use std::path::Path;
use serde_json as serde;
use rouille;

use queue::{Queue, NewQueueEntry};
use config::ApiConfig;

pub type Listeners = Arc<Mutex<HashMap<usize, Listener>>>;
type SQueue = Arc<Mutex<Queue>>;
type ApiChan = Arc<Mutex<Sender<ApiMessage>>>;

struct Server {
    queue: SQueue,
    listeners: Listeners,
    chan: ApiChan,
}

#[derive(Debug)]
pub enum QueuePos {
    Head,
    Tail,
}

#[derive(Debug)]
pub enum ApiMessage {
    Skip,
    Remove(QueuePos),
    Insert(QueuePos, NewQueueEntry),
    Clear,
}

#[derive(Serialize)]
pub struct Resp {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Serialize)]
pub struct Listener {
    pub mount: String,
    pub path: String,
    pub headers: Vec<Header>,
}

#[derive(Serialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl Server {
    fn handle_request(&self, req: &rouille::Request) -> rouille::Response {
        router!(req,
                (GET) (/np) => {
                    debug!("Handling now playing req");
                    let q = self.queue.lock().unwrap();
                    rouille::Response::from_data(
                        "application/json",
                        serde::to_string(&q.np().entry().serialize()).unwrap())
                },

                (GET) (/listeners) => {
                    debug!("Handling listeners req");
                    let l = self.listeners.lock().unwrap();
                    rouille::Response::from_data(
                        "application/json",
                        serde::to_string::<Vec<&Listener>>(&l.iter().map(|(_, v)| v).collect()).unwrap())
                },

                (GET) (/queue) => {
                    debug!("Handling queue disp req");
                    let q = self.queue.lock().unwrap();
                    rouille::Response::from_data(
                        "application/json",
                        serde::to_string(&q.entries().iter().map(|e| e.serialize()).collect::<Vec<_>>()).unwrap())
                },

                (POST) (/queue/head) => {
                    match serde::from_reader(req.data().unwrap()).map(|d| NewQueueEntry::deserialize(d)) {
                        Ok(Some(qe)) => {
                            debug!("Handling queue head insert");
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
                        Ok(None) => {
                            rouille::Response::from_data(
                                "application/json",
                                serde::to_string(&Resp::failure("blob must contain path!")).unwrap()
                            ).with_status_code(400)
                        }
                        Err(_) => {
                            rouille::Response::from_data(
                                "application/json",
                                serde::to_string(&Resp::failure("malformed json sent")).unwrap()
                            ).with_status_code(400)
                        }
                    }
                },

                (DELETE) (/queue/head) => {
                    debug!("Handling queue head remove");
                    self.chan.lock().unwrap().send(ApiMessage::Remove(QueuePos::Head)).unwrap();
                    rouille::Response::from_data(
                        "application/json",
                        serde::to_string(&Resp::success()).unwrap())
                },

                (POST) (/queue/tail) => {
                    debug!("Handling queue tail insert");
                    match serde::from_reader(req.data().unwrap()).map(|d| NewQueueEntry::deserialize(d)) {
                        Ok(Some(qe)) => {
                            debug!("Handling queue head insert");
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
                        Ok(None) => {
                            rouille::Response::from_data(
                                "application/json",
                                serde::to_string(&Resp::failure("blob must contain path!")).unwrap()
                            ).with_status_code(400)
                        }
                        Err(_) => {
                            rouille::Response::from_data(
                                "application/json",
                                serde::to_string(&Resp::failure("malformed json sent")).unwrap()
                            ).with_status_code(400)
                        }
                    }
                },

                (DELETE) (/queue/tail) => {
                    debug!("Handling queue tail remove");
                    self.chan.lock().unwrap().send(ApiMessage::Remove(QueuePos::Tail)).unwrap();
                    rouille::Response::from_data(
                        "application/json",
                        serde::to_string(&Resp::success()).unwrap())
                },

                (POST) (/skip) => {
                    debug!("Handling queue skip");
                    self.chan.lock().unwrap().send(ApiMessage::Skip).unwrap();
                    rouille::Response::from_data(
                        "application/json",
                        serde::to_string(&Resp::success()).unwrap())
                },

                (POST) (/queue/clear) => {
                    debug!("Handling queue clear");
                    self.chan.lock().unwrap().send(ApiMessage::Clear).unwrap();
                    rouille::Response::from_data(
                        "application/json",
                        serde::to_string(&Resp::success()).unwrap())
                },

                _ => rouille::Response::empty_404()
            )
    }
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


pub fn start_api(config: ApiConfig, queue: Arc<Mutex<Queue>>, listeners: Listeners, updates: Sender<ApiMessage>) {
    thread::spawn(move || {
        info!("Starting API");
        let chan = Arc::new(Mutex::new(updates));
        let serv = Server {
            queue: queue,
            chan: chan,
            listeners,
        };
        rouille::start_server(("127.0.0.1", config.port), move |request| {
            serv.handle_request(request)
        });
    });
}
