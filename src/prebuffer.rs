use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use shout;
use config::StreamConfig;
use ring_buffer::RingBuffer;
use transcode;

pub struct PreBuffer {
    pub buffer: Arc<RingBuffer<u8>>,
    pub token: Arc<AtomicBool>,
}

impl PreBuffer {
    pub fn from_transcode(input: Arc<Vec<u8>>, ext: &str, cfg: &StreamConfig) -> Option<PreBuffer> {
        let token = Arc::new(AtomicBool::new(false));
        // 500KB Buffer
        let out_buf = Arc::new(RingBuffer::new(500000));
        let res_ext = match cfg.container {
            shout::ShoutFormat::Ogg => "ogg",
            shout::ShoutFormat::MP3 => "mp3",
            _ => {
                return None;
            }
        };
        if let Err(e) = transcode::transcode(input,
                                             ext,
                                             out_buf.clone(),
                                             res_ext,
                                             cfg.codec,
                                             cfg.bitrate,
                                             token.clone()) {
            println!("WARNING: Transcoder creation failed with error: {}", e);
            None
        } else {
            Some(PreBuffer {
                buffer: out_buf,
                token: token,
            })
        }
    }

    pub fn cancel(&mut self) {
        self.token.store(true, Ordering::SeqCst);
    }
}

impl Drop for PreBuffer {
    fn drop(&mut self) {
        self.cancel();
    }
}
