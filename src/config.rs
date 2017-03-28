use ffmpeg::codec::id::Id;
use shout::ShoutFormat;
use toml;

use std::sync::Arc;
use super::util;

#[derive(Clone)]
pub struct Config {
    pub api: ApiConfig,
    pub radio: RadioConfig,
    pub streams: Vec<StreamConfig>,
    pub queue: QueueConfig,
}

#[derive(Clone)]
pub struct StreamConfig {
    pub mount: String,
    pub bitrate: Option<usize>,
    pub container: ShoutFormat,
    pub codec: Id,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadioConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiConfig {
    pub port: u16,
}

#[derive(Clone)]
pub struct QueueConfig {
    pub random: String,
    pub fallback: (Arc<Vec<u8>>, String),
}

// Some unfortunate code duplication because you can't derive Deserialize for newtypes in this case
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InternalConfig {
    pub api: ApiConfig,
    pub radio: RadioConfig,
    pub streams: Vec<InternalStreamConfig>,
    pub queue: InternalQueueConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InternalStreamConfig {
    pub mount: String,
    pub bitrate: Option<usize>,
    pub container: String,
    pub codec: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InternalQueueConfig {
    #[serde(rename = "random_song_api")]
    pub random: String,
    pub fallback: String,
}

impl InternalConfig {
    fn into_config(self) -> Result<Config, String> {
        // TODO: Should be alloca'ed, but w/e
        let mut streams = Vec::with_capacity(self.streams.len());
        for s in self.streams {
            let container = match &*s.container {
                "ogg" => ShoutFormat::Ogg,
                "mp3" => ShoutFormat::MP3,
                _ => return Err(format!("Currently, only ogg and mp3 are supported as containers.")),
            };
            let codec = if let Some(c) = s.codec {
                match &*c {
                    "opus" => Id::OPUS_DEPRECATED,
                    "vorbis" => Id::VORBIS,
                    "flac" => Id::FLAC,
                    "mp3" => Id::MP3,
                    _ => return Err(format!("Currently, only opus, vorbis, flac, and mp3 are \
                                            supported as codecs.")),
                }
            } else {
                Id::MP3
            };

            streams.push(StreamConfig {
                             mount: s.mount,
                             bitrate: s.bitrate,
                             container: container,
                             codec: codec,
                         })
        }

        Ok(Config {
               api: self.api,
               radio: self.radio,
               streams: streams,
               queue: QueueConfig {
                    random: self.queue.random,
                    fallback: util::path_to_data(&self.queue.fallback)
                                        .expect("Queue fallback must be present and a valid file!"),
               },
           })
    }
}

pub fn parse_config(input: &str) -> Result<Config, String> {
    let parsed: Result<InternalConfig, _> = toml::de::from_str(input);
    if let Err(e) = parsed {
        Err(format!("{}", e))
    } else {
        parsed.unwrap().into_config()
    }
}
