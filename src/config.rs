use shout::ShoutFormat;
use toml;
use kaeru::AVCodecID;

use std::sync::Arc;
use std::fs::File;
use std::io::Read;

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
    pub bitrate: Option<i64>,
    pub container: ShoutFormat,
    pub codec: AVCodecID,
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
                    "opus" => AVCodecID::AV_CODEC_ID_OPUS,
                    "vorbis" => AVCodecID::AV_CODEC_ID_VORBIS,
                    "flac" => AVCodecID::AV_CODEC_ID_FLAC,
                    "mp3" => AVCodecID::AV_CODEC_ID_MP3,
                    _ => return Err(format!("Currently, only opus, vorbis, flac, and mp3 are \
                                            supported as codecs.")),
                }
            } else {
                AVCodecID::AV_CODEC_ID_OPUS
            };

            streams.push(StreamConfig {
                             mount: s.mount,
                             bitrate: s.bitrate.map(|b| b as i64),
                             container: container,
                             codec: codec,
                         })
        }

        let mut buffer = Vec::new();
        File::open(&self.queue.fallback).expect("Queue fallback must be present and a vaild file").read_to_end(&mut buffer).expect("IO ERROR!");
        let fbp = self.queue.fallback.split('.').last().expect("Queue fallback must have a container extension");
        if fbp != "ogg" && fbp != "mp3" {
            panic!("Fallback must be mp3 or ogg");
        }
        Ok(Config {
               api: self.api,
               radio: self.radio,
               streams: streams,
               queue: QueueConfig {
                    random: self.queue.random,
                    fallback: (Arc::new(buffer), fbp.to_owned()),
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
