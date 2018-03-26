use std::sync::Arc;
use kaeru::Metadata;
use tc_queue;

pub struct PreBuffer {
    pub buffer: tc_queue::QR,
    pub metadata: Arc<Metadata>,
}

impl PreBuffer {
    pub fn new(buffer: tc_queue::QR, md: Arc<Metadata>) -> PreBuffer {
        PreBuffer {
            buffer,
            metadata: md,
        }
    }
}
