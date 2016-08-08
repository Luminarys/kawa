use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
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
            if conn.delay() == 0 {
                thread::sleep(Duration::from_millis(100));
            } else {
                conn.sync();
            }
        }
    }
}
