use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::fs::File;
use std::io::{Read, self};
use ring_buffer::RingBuffer;
use CPU_POOL;

pub fn pair_opt_map<T, U>(p: (Option<T>, Option<U>)) -> Option<(T, U)> {
    match p {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    }
}

pub fn path_to_rb(path: &str) -> Option<(Arc<RingBuffer<u8>>, String)> {
    let ext = Path::new(path)
        .extension()
        .map(|e| e.to_os_string())
        .and_then(|s| s.into_string().ok());

    let buf = Arc::new(RingBuffer::new(500000));
    let local_buf = buf.clone();
    let b = File::open(path)
        .ok()
        .map(|mut f| {
            CPU_POOL.spawn_fn(move || {
                let mut data = Vec::new();
                let amnt = f.read_to_end(&mut data)?;
                buf.write(&data[0..amnt]);
                // T Y P E I N F E R E N C E
                if false {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            }).forget();
        })
        .map(|_| local_buf);

    pair_opt_map((b, ext))
}

pub fn data_to_rb(data: Arc<Vec<u8>>) -> Arc<RingBuffer<u8>> {
    let rb = Arc::new(RingBuffer::new(500000));
    let rbc = rb.clone();
    CPU_POOL.spawn_fn(move || {
        rbc.write(&data);
        // T Y P E I N F E R E N C E
        if false {
            return Err(());
        }
        Ok(())
    }).forget();
    rb
}

// Multiplexes an input ringbuffer to multiple outputs, with a cancellation
// token that can be used to prematurely stop the process.
pub fn rb_broadcast(input: Arc<RingBuffer<u8>>, outputs: Vec<Arc<RingBuffer<u8>>>, compl: Arc<AtomicBool>) {
    CPU_POOL.spawn_fn(move || {
        loop {
            if compl.load(Ordering::Acquire) {
                break;
            }
            let buf = input.try_read(1024);
            for output in outputs.iter() {
                output.write(&buf);
            }
        }
        // T Y P E I N F E R E N C E
        if false {
            return Err(());
        }
        Ok(())
    }).forget();
}
