#[derive(Default, RustcDecodable, RustcEncodable)]
pub struct Queue {
    pub entries: Vec<QueueEntry>,
}

#[derive(RustcDecodable, RustcEncodable)]
pub struct QueueEntry {
    pub id: String,
    pub path: String,
}

impl QueueEntry {
    pub fn new(id: String, path: String) -> QueueEntry {
        QueueEntry {
            id: id,
            path: path,
        }
    }
}
