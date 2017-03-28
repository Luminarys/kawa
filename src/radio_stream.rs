use file_stream::FileStream;
use futures::{Async, Future, Stream, Poll, future, BoxFuture};
use ffmpeg::{self, format, codec, media, frame, filter};
use std::io;

// Even though this is theoretically how to model
// the entire process as Futures, practically
// we should just separate out the Decode process
// and the Encode Process
// struct DecodeBytes;
// struct DecodePackets;
// struct TranscodeFrames;
// struct EncodePackets;
// struct EncodeBytes;

struct DecodeStream;
struct EncodeStream;

impl DecodeStream {
    fn feed(&self, buf: Vec<u8>) -> BoxFuture<Vec<frame::Audio>, io::Error> {
        unimplemented!();
    }
}

impl EncodeStream {
    fn feed(&self, frame: Vec<frame::Audio>) -> BoxFuture<Vec<u8>, io::Error> {
        unimplemented!();
    }
}

fn run_streams() {
    let file = String::from("");
    loop {
        let fs = FileStream::new(file.clone());
        let transcoders: Vec<EncodeStream> = Vec::new();
        let input_tc = DecodeStream;

        fs.and_then(|(v, a)| {
            input_tc.feed(v)
        }).and_then(|frames| {
            let mut futs = Vec::new();
            for tc in transcoders.iter() {
                // TODO: Use an ARC
                futs.push(tc.feed(frames.clone()));
            }
            future::join_all(futs)
        })
    }
}
