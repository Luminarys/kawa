use slog::Logger;
use ring_buffer::RBReader;
use std::sync::Arc;
use kaeru::Metadata;

#[derive(Clone)]
pub struct PreBuffer {
    pub buffer: RBReader,
    pub metadata: Arc<Metadata>,
    pub log: Logger,
}

impl PreBuffer {
    pub fn new(buffer: RBReader, md: Arc<Metadata>, log: Logger) -> PreBuffer {
        PreBuffer {
            buffer,
            log,
            metadata: md,
        }
    }
}
