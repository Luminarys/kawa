use std::sync::{Arc, Mutex};
use std::sync::mpsc::Sender;
use std::thread;

use queue::{Queue, QueueEntry};
use config::ApiConfig;
use rustful::{Server, Handler, Context, Response, TreeRouter, StatusCode};
use rustc_serialize::json;

pub enum ApiMessage {
    Skip,
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
            Some("/queue/add") => {
                match (context.query.get("id"), context.query.get("path")) {
                    // :')))
                    (Some(id), Some(path)) => {
                        let mut q = self.queue.lock().unwrap();
                        q.entries.push(QueueEntry::new(id.into_owned(), path.into_owned()));
                        response.send("{success:\"true\"}");
                    }
                    _ => {
                        response.set_status(StatusCode::BadRequest);
                        response.send("{failure:\"true\", reason:\"missing parameters\"}");
                    }
                }
            }
            Some("/skip") => {
                self.updates.lock().unwrap().send(ApiMessage::Skip).unwrap();
                response.send("{success:\"true\"}");
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
                },
                "skip" => {
                    Get: context.clone(),
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
