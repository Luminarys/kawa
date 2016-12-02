use std::path::Path;
use std::sync::Arc;
use std::fs::File;
use std::io::Read;

pub fn pair_opt_map<T, U>(p: (Option<T>, Option<U>)) -> Option<(T, U)> {
    match p {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    }
}

pub fn path_to_data(path: &str) -> Option<(Arc<Vec<u8>>, String)> {
    let ext = Path::new(path)
        .extension()
        .map(|e| e.to_os_string())
        .and_then(|s| s.into_string().ok());

    let mut in_buf = Vec::new();
    let buf = File::open(path)
        .ok()
        .and_then(|mut f| f.read_to_end(&mut in_buf).ok())
        .map(|_| Arc::new(in_buf));

    pair_opt_map((buf, ext))
}
