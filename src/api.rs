use std::sync::{Arc, Mutex};
use std::sync::mpsc::Sender;
use std::thread;
use std::path::Path;
use rustful::{Server, Context, Response, TreeRouter, StatusCode};
use rustful::server::Global;
use rustc_serialize::json;
use slog::Logger;

use util;
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

struct SData {
    queue: SQueue,
    chan: ApiChan,
    log: Logger,
}

fn body_to_qe(context: &mut Context) -> Result<QueueEntry, &'static str> {
    context.body
        .read_json_body()
        .map_err(|_| "Body must be JSON formatted")
        .and_then(|body| {
            let id = body.find("id").and_then(|j| j.as_i64());
            let path = body.find("path").and_then(|j| j.as_string()).map(|s| s.to_owned());
            util::pair_opt_map((id, path)).ok_or("Body must contain id and path keys")
        })
        .map(|(id, path)| QueueEntry::new(id.to_owned(), path.to_owned()))
        .and_then(|qe| {
            if Path::new(&qe.path).exists() {
                Ok(qe)
            } else {
                Err("Song file must exist on disk")
            }
        })
}

fn send_api_message(ctx: Context, resp: Response, msg: ApiMessage) {
    let sdata = ctx.global.get::<SData>().unwrap();
    debug!(sdata.log, "Sending message to radio ctrl: {:?}", msg);
    sdata.chan.lock().unwrap().send(msg).unwrap();
    resp.send(json::encode(&Resp::success()).unwrap());
}

fn queue_view(ctx: Context, response: Response) {
    let sdata = ctx.global.get::<SData>().unwrap();
    let q = sdata.queue.lock().unwrap();
    if let Ok(resp) = json::encode(&q.entries) {
        response.send(resp);
    }
}

fn queue_head_insert(mut ctx: Context, mut resp: Response) {
    match body_to_qe(&mut ctx) {
        Ok(qe) => {
            send_api_message(ctx, resp, ApiMessage::Insert(QueuePos::Head, qe));
        }
        Err(reason) => {
            resp.set_status(StatusCode::BadRequest);
            resp.send(json::encode(&Resp::failure(reason)).unwrap());
        }
    };
}

fn queue_head_delete(ctx: Context, resp: Response) {
    send_api_message(ctx, resp, ApiMessage::Remove(QueuePos::Head));
}

fn queue_tail_insert(mut ctx: Context, mut resp: Response) {
    match body_to_qe(&mut ctx) {
        Ok(qe) => {
            send_api_message(ctx, resp, ApiMessage::Insert(QueuePos::Tail, qe));
        }
        Err(reason) => {
            resp.set_status(StatusCode::BadRequest);
            resp.send(json::encode(&Resp::failure(reason)).unwrap());
        }
    };
}

fn queue_tail_delete(ctx: Context, resp: Response) {
    send_api_message(ctx, resp, ApiMessage::Remove(QueuePos::Tail));
}

fn queue_clear(ctx: Context, resp: Response) {
    send_api_message(ctx, resp, ApiMessage::Clear);
}

fn queue_skip(ctx: Context, resp: Response) {
    send_api_message(ctx, resp, ApiMessage::Skip);
}

pub fn start_api(config: ApiConfig, queue: Arc<Mutex<Queue>>, updates: Sender<ApiMessage>, log: Logger) {
    thread::spawn(move || {
        info!(log, "Starting API");
        let chan = Arc::new(Mutex::new(updates));
        let sdata = SData {
            queue: queue,
            chan: chan,
            log: log,
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
