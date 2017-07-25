use slog::Logger;
use std::sync::Arc;
use kaeru::Metadata;
use tc_queue;

#[derive(Clone)]
pub struct PreBuffer {
    pub buffer: tc_queue::QR,
    pub metadata: Arc<Metadata>,
    pub log: Logger,
}

impl PreBuffer {
    pub fn new(buffer: tc_queue::QR, md: Arc<Metadata>, log: Logger) -> PreBuffer {
        PreBuffer {
            buffer,
            log,
            metadata: md,
        }
    }
}
