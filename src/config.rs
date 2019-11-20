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
    pub container: Container,
    pub codec: AVCodecID::Type,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadioConfig {
    pub port: u16,
    pub name: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiConfig {
    pub port: u16,
}

#[derive(Clone)]
pub struct QueueConfig {
    pub random: String,
    pub np: String,
    pub fallback: (Arc<Vec<u8>>, String),
    pub buffer_len: usize,
}

#[derive(Clone)]
pub enum Container {
    Ogg,
    MP3,
    AAC,
    FLAC,
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
    pub np: String,
    pub fallback: String,
    pub buffer_len: usize,
}

impl InternalConfig {
    fn into_config(self) -> Result<Config, String> {
        // TODO: Should be alloca'ed, but w/e
        let mut streams = Vec::with_capacity(self.streams.len());
        for s in self.streams {
            let container = match &*s.container {
                "ogg" => Container::Ogg,
                "mp3" => Container::MP3,
                "aac" => Container::AAC,
                "flac" => Container::FLAC,
                _ => return Err(format!("Currently, only ogg, mp3, aac, and flac are supported as containers.")),
            };
            let codec = if let Some(c) = s.codec {
                match &*c {
                    "opus" => AVCodecID::AV_CODEC_ID_OPUS,
                    "vorbis" => AVCodecID::AV_CODEC_ID_VORBIS,
                    "flac" => AVCodecID::AV_CODEC_ID_FLAC,
                    "mp3" => AVCodecID::AV_CODEC_ID_MP3,
                    "aac" => AVCodecID::AV_CODEC_ID_AAC,
                    _ => return Err(format!("Currently, only opus, vorbis, flac, aac, and mp3 are \
                                            supported as codecs.")),
                }
            } else {
                // Default to OPUS for Ogg, and MP3 for MP3
                match container {
                    Container::Ogg => AVCodecID::AV_CODEC_ID_OPUS,
                    Container::MP3 => AVCodecID::AV_CODEC_ID_MP3,
                    Container::AAC => AVCodecID::AV_CODEC_ID_AAC,
                    Container::FLAC => AVCodecID::AV_CODEC_ID_FLAC,
                }
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
        if fbp != "ogg" && fbp != "mp3" && fbp != "flac" {
            panic!("Fallback must be mp3 or ogg or flac");
        }
        Ok(Config {
               api: self.api,
               radio: self.radio,
               streams: streams,
               queue: QueueConfig {
                    random: self.queue.random,
                    np: self.queue.np,
                    fallback: (Arc::new(buffer), fbp.to_owned()),
                    buffer_len: self.queue.buffer_len,
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
