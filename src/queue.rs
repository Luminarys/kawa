#[derive(Default, RustcDecodable, RustcEncodable)]
pub struct Queue {
    pub entries: Vec<QueueEntry>,
}

impl Queue {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn push(&mut self, qe: QueueEntry) {
        self.entries.push(qe);
    }

    pub fn pop(&mut self) -> Option<QueueEntry> {
        self.entries.pop()
    }

    pub fn insert(&mut self, index: usize, qe: QueueEntry) {
        self.entries.insert(index, qe);
    }

    pub fn remove(&mut self, index: usize) -> QueueEntry {
        self.entries.remove(index)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
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
