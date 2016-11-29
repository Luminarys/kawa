use std::sync::{Arc, Mutex};
use std::sync::mpsc::Sender;
use std::thread;
use std::path::Path;

use queue::{Queue, QueueEntry};
use config::ApiConfig;
use rustful::{Server, Handler, Context, Response, TreeRouter, StatusCode};
use rustful::server::Global;
use rustc_serialize::json;

type SQueue = Arc<Mutex<Queue>>;
type ApiChan = Arc<Mutex<Sender<ApiMessage>>>;

struct SData {
    queue: SQueue,
    chan: ApiChan,
}

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

fn pair_opt_to_opt_pair<T, U>(p: (Option<T>, Option<U>)) -> Option<(T, U)> {
    match p {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    }
}

fn body_to_qe(mut context: Context) -> Result<QueueEntry, &'static str> {
    context.body
        .read_json_body()
        .map_err(|_| "Body must be JSON formatted")
        .and_then(|body| {
            let id = body.find("id").and_then(|j| j.as_string()).map(|s| s.to_owned());
            let path = body.find("path").and_then(|j| j.as_string()).map(|s| s.to_owned());
            pair_opt_to_opt_pair((id, path)).ok_or("Body must contain id and path keys")
        })
        .map(|(id, path)| QueueEntry::new(id.to_owned(), path.to_owned()))
        .and_then(|qe| {
            if Path::new(&qe.path).exists() {
                Err("Song file must exist on disk")
            } else {
                Ok(qe)
            }
        })
}

fn queue_view(ctx: Context, response: Response) {
    let sdata = ctx.global.get::<SData>().unwrap();
    let q = sdata.queue.lock().unwrap();
    if let Ok(resp) = json::encode(&q.entries) {
        response.send(resp);
    }
}

fn queue_head_insert(ctx: Context, mut response: Response) {
    let sdata = ctx.global.get::<SData>().unwrap();
    match body_to_qe(ctx) {
        Ok(qe) => {
            sdata.chan.lock().unwrap().send(ApiMessage::Insert(QueuePos::Head, qe)).unwrap();
            response.send(json::encode(&Resp::success()).unwrap());
        }
        Err(reason) => {
            response.set_status(StatusCode::BadRequest);
            response.send(json::encode(&Resp::failure(reason)).unwrap());
        }
    };
}

fn queue_head_delete(ctx: Context, response: Response) {
    let sdata = ctx.global.get::<SData>().unwrap();
    sdata.chan.lock().unwrap().send(ApiMessage::Remove(QueuePos::Head)).unwrap();
    response.send(json::encode(&Resp::success()).unwrap());
}

fn queue_tail_insert(ctx: Context, mut response: Response) {
    let sdata = ctx.global.get::<SData>().unwrap();
    match body_to_qe(ctx) {
        Ok(qe) => {
            sdata.chan.lock().unwrap().send(ApiMessage::Insert(QueuePos::Tail, qe)).unwrap();
            response.send(json::encode(&Resp::success()).unwrap());
        }
        Err(reason) => {
            response.set_status(StatusCode::BadRequest);
            response.send(json::encode(&Resp::failure(reason)).unwrap());
        }
    };
}

fn queue_tail_delete(ctx: Context, response: Response) {
    let sdata = ctx.global.get::<SData>().unwrap();
    sdata.chan.lock().unwrap().send(ApiMessage::Remove(QueuePos::Tail)).unwrap();
    response.send(json::encode(&Resp::success()).unwrap());
}

fn queue_clear(ctx: Context, response: Response) {
    let sdata = ctx.global.get::<SData>().unwrap();
    sdata.chan.lock().unwrap().send(ApiMessage::Clear).unwrap();
    response.send(json::encode(&Resp::success()).unwrap());
}

fn queue_skip(ctx: Context, response: Response) {
    let sdata = ctx.global.get::<SData>().unwrap();
    sdata.chan.lock().unwrap().send(ApiMessage::Skip).unwrap();
    response.send(json::encode(&Resp::success()).unwrap());
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
            Some("/queue/insert/head") |
            Some("/queue/insert/tail") => {

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
                            response.send(json::encode(&Resp::failure("Nonexistent file specified"))
                                    .unwrap());
                        }
                    }
                    _ => {
                        response.set_status(StatusCode::BadRequest);
                        response.send(json::encode(&Resp::failure("missing parameters")).unwrap());
                    }
                }
            }
            Some("/queue/remove/head") |
            Some("/queue/remove/tail") => {
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
        let chan = Arc::new(Mutex::new(updates));
        let sdata = SData {
            queue: queue,
            chan: chan,
        };
        let gl: Global = Box::new(sdata).into();
        let router = insert_routes!{
            TreeRouter::new() => {
                "queue" => {
                    Get: queue_view as fn(Context, Response),
                    "/head" => { Post: queue_head_insert, Delete: queue_head_delete },
                    "/tail" => { Post: queue_tail_insert, Delete: queue_tail_delete },
                    "/clear" => Post: queue_clear,
                    "/skip" => Post: queue_skip,
                }
            }
        };
        let server = Server {
            handlers: router,
            global: gl,
            host: config.port.into(),
            ..Server::default()
        };
        println!("Starting API");
        server.run().unwrap();
        println!("thread over?");
    });
}
