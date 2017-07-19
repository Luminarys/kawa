use slog::Logger;
use ring_buffer::RBReader;

#[derive(Clone)]
pub struct PreBuffer {
    pub buffer: RBReader,
    pub log: Logger,
}
