use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use slog::Logger;
use shout;
use config::StreamConfig;
use ring_buffer::RingBuffer;
use transcode;

pub struct PreBuffer {
    pub buffer: Arc<RingBuffer<u8>>,
    pub token: Arc<AtomicBool>,
    log: Logger,
}

impl PreBuffer {
    pub fn from_transcode(input: Arc<Vec<u8>>, ext: &str, cfg: &StreamConfig, log: Logger) -> Option<PreBuffer> {
        let token = Arc::new(AtomicBool::new(false));
        // 500KB Buffer
        let out_buf = Arc::new(RingBuffer::new(500000));
        let res_ext = match cfg.container {
            shout::ShoutFormat::Ogg => "ogg",
            shout::ShoutFormat::MP3 => "mp3",
            _ => {
                unreachable!();
            }
        };
        if let Err(e) = transcode::transcode(input,
                                             ext,
                                             out_buf.clone(),
                                             res_ext,
                                             cfg.codec,
                                             cfg.bitrate,
                                             token.clone(),
                                             log.clone()) {
            warn!(log, "Transcoder creation failed: {}", e);
            None
        } else {
            Some(PreBuffer {
                buffer: out_buf,
                token: token,
                log: log,
            })
        }
    }

    pub fn cancel(&mut self) {
        debug!(self.log, "Cancelling transcode!");
        self.token.store(true, Ordering::SeqCst);
    }
}

impl Drop for PreBuffer {
    fn drop(&mut self) {
        self.cancel();
    }
}
