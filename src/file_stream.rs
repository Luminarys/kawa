use futures::{Async, Future, Stream, Poll};
use futures_cpupool::{CpuFuture};
use std::fs::{self, File};
use std::io::prelude::*;
use std::io;
use std::sync::{Arc, Mutex};
use CPU_POOL;

const BUF_SIZE: usize = 1024;

pub struct FileStream {
    file: Arc<Mutex<File>>,
    current: CpuFuture<(Vec<u8>, usize), io::Error>,
}

impl FileStream {
    pub fn new(file: String) -> FileStream {
        let file = Arc::new(Mutex::new(File::open(&file).unwrap()));
        let fc = file.clone();
        let fut = CPU_POOL.spawn_fn(move || {
            let mut buf = vec![0u8; BUF_SIZE];
            let amnt = fc.lock().unwrap().read(&mut buf)?;
            let res= Ok((buf, amnt));
            res
        });
        FileStream {
            file: file,
            current: fut
        }
    }
}

impl Stream for FileStream {
    type Item = (Vec<u8>, usize);
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let mut compl = false;
        let res = self.current.poll().map(|a| {
            match a {
                Async::Ready((v, a)) => {
                    if a == 0 {
                        Async::Ready(None)
                    } else {
                        compl = true;
                        Async::Ready(Some((v, a)))
                    }
                },
                Async::NotReady => Async::NotReady,
            }
        });
        if compl {
            let fc = self.file.clone();
            self.current = CPU_POOL.spawn_fn(move || {
                let mut buf = vec![0u8; BUF_SIZE];
                let amnt = fc.lock().unwrap().read(&mut buf)?;
                let res = Ok((buf, amnt));
                res
            });
        }
        res
    }
}

#[test]
fn test_file_stream() {
    const COUNT: usize = 10;
    let mut file = File::create("foo.txt").unwrap();
    for i in 0..COUNT {
        file.write(&mut vec![1u8; 1025]).unwrap();
    }
    let f = FileStream::new(String::from("foo.txt")).wait();
    for i in f {
        let (v, a) = i.unwrap();
        assert!(a == 1024 || a == COUNT);
    }
    fs::remove_file("foo.txt").unwrap();
}
