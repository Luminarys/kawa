use std::sync::{Arc, Mutex};
use shout;

pub fn play(conn: shout::ShoutConn, buffer: Arc<Mutex<Vec<u8>>>) {
    let step = 4096;
    loop {
        let mut data = buffer.lock().unwrap();
        if step < data.len() {
            conn.send(data.drain(0..step).collect());
            drop(data);
            conn.sync();
        } else {
            conn.send(data.drain(..).collect());
            conn.sync();
        }
    }
}
