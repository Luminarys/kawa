use std::sync::{Arc, Mutex};
use std::sync::mpsc::Sender;
use std::thread;
use std::path::Path;

use queue::{Queue, QueueEntry};
use config::ApiConfig;
use rustful::{Server, Handler, Context, Response, TreeRouter, StatusCode};
use rustc_serialize::json;

pub enum QueuePos {
    Head,
    Tail,
}

pub enum ApiMessage {
    Skip,
    Remove(QueuePos),
    Insert(QueuePos, QueueEntry),
    Clear,
}

#[derive(RustcEncodable)]
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


#[derive(Clone)]
struct StreamerApi {
    queue: Arc<Mutex<Queue>>,
    updates: Arc<Mutex<Sender<ApiMessage>>>,
}

impl Handler for StreamerApi {
    fn handle_request(&self, context: Context, mut response: Response) {
        match context.uri.as_utf8_path() {
            Some("/queue") => {
                let q = self.queue.lock().unwrap();
                if let Ok(resp) = json::encode(&q.entries) {
                    response.send(resp);
                }
            }
            Some("/queue/insert/head") | Some("/queue/insert/tail") => {
                match (context.query.get("id"), context.query.get("path")) {
                    // :')))
                    (Some(id), Some(path)) => {
                        if Path::new(&path.clone().into_owned()).exists() {
                            let qe = QueueEntry::new(id.into_owned(), path.into_owned());
                            let pos = if context.uri.as_utf8_path() == Some("/queue/insert/head") {
                                QueuePos::Head
                            } else {
                                QueuePos::Tail
                            };
                            self.updates.lock().unwrap().send(ApiMessage::Insert(pos, qe)).unwrap();
                            response.send(json::encode(&Resp::success()).unwrap());
                        } else {
                            response.send(json::encode(&Resp::failure("Nonexistent file specified")).unwrap());
                        }
                    }
                    _ => {
                        response.set_status(StatusCode::BadRequest);
                        response.send(json::encode(&Resp::failure("missing parameters")).unwrap());
                    }
                }
            }
            Some("/queue/remove/head") | Some("/queue/remove/tail") => {
                let pos = if context.uri.as_utf8_path() == Some("/queue/remove/head") {
                    QueuePos::Head
                } else {
                    QueuePos::Tail
                };
                self.updates.lock().unwrap().send(ApiMessage::Remove(pos)).unwrap();
                response.send(json::encode(&Resp::success()).unwrap());

            }
            Some("/queue/clear") => {
                self.updates.lock().unwrap().send(ApiMessage::Clear).unwrap();
                response.send(json::encode(&Resp::success()).unwrap());
            }
            Some("/queue/skip") => {
                self.updates.lock().unwrap().send(ApiMessage::Skip).unwrap();
                response.send(json::encode(&Resp::success()).unwrap());
            }
            Some(p) => {
                println!("Unknown path {:?}", p);
            }
            None => {
                println!("Bad path!");
            }
        }
    }
}

pub fn start_api(config: ApiConfig, queue: Arc<Mutex<Queue>>, updates: Sender<ApiMessage>) {
    thread::spawn(move || {
        let context = StreamerApi {
            queue: queue,
            updates: Arc::new(Mutex::new(updates)),
        };
        let router = insert_routes!{
            TreeRouter::new() => {
                "queue" => {
                    Get: context.clone(),
                    "/add" => Post: context.clone(),
                    "/insert/head" => Post: context.clone(),
                    "/insert/tail" => Post: context.clone(),
                    "/remove/head" => Get: context.clone(),
                    "/remove/tail" => Get: context.clone(),
                    "/clear" => Get: context.clone(),
                    "skip" => {
                        Get: context.clone(),
                    }
                }
            }
        };
        let server = Server {
            handlers: router,
            host: config.port.into(),
            ..Server::default()
        };
        println!("Starting API");
        server.run().unwrap();
        println!("thread over?");
    });
}
