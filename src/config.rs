use toml;
use ffmpeg::codec;
use shout::ShoutFormat;
use hyper::Url;

pub struct Config {
    pub streams: Vec<StreamConfig>,
    pub radio: RadioConfig,
    pub api: ApiConfig,
}

pub struct StreamConfig {
    pub mount: String,
    pub bitrate: Option<usize>,
    pub container: ShoutFormat,
    pub codec: codec::id::Id,
}

pub struct RadioConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

pub struct ApiConfig {
    pub port: u16,
    pub remote_random: Url,
}

pub fn parse_config(input: String) -> Result<Config, String> {
    let mut parser = toml::Parser::new(&input);
    let toml = match parser.parse() {
        Some(toml) => toml,
        None => {
            for err in &parser.errors {
                let (loline, locol) = parser.to_linecol(err.lo);
                let (hiline, hicol) = parser.to_linecol(err.hi);
                return Err(format!("{}:{}-{}:{} error: {}",
                                   loline,
                                   locol,
                                   hiline,
                                   hicol,
                                   err.desc));
            }
            return Err(format!("Unknown error parsing config!"));
        }
    };

    let streams = try!(parse_streams(toml.clone()));
    let radio = try!(parse_radio(toml.clone()));
    let api = try!(parse_api(toml.clone()));
    Ok(Config {
        streams: streams,
        radio: radio,
        api: api,
    })
}

fn parse_streams(input: toml::Table) -> Result<Vec<StreamConfig>, String> {
    match input.remove("streams") {
        Some(value) => {
            let mut streams = Vec::new();
            if let Some(slice) = value.as_slice() {
                for stream in slice {
                    streams.push(try!(parse_stream(stream.clone())));
                }
                Ok(streams)
            } else {
                Err(format!("The streams portion of the config must be an array!"))
            }
        }
        None => Err(format!("Config must contain streams!")),
    }
}

fn parse_stream(input: toml::Value) -> Result<StreamConfig, String> {
    if let Some(table) = input.as_table() {
        let container = if let Some(value) = table.remove("container") {
            match value.as_str() {
                Some("ogg") => ShoutFormat::Ogg,
                Some("mp3") => ShoutFormat::MP3,
                _ => {
                    return Err(format!("Only ogg and mp3 containers are supported for now, {:?} \
                                        is an invalid stream container.",
                                       value))
                }
            }
        } else {
            return Err(format!("Stream entries must contain a container field!"));
        };

        let codec = if let Some(value) = table.remove("codec") {
            match value.as_str() {
                Some("opus") => codec::id::Id::OPUS,
                Some("vorbis") => codec::id::Id::VORBIS,
                Some("flac") => codec::id::Id::FLAC,
                Some("mp3") => codec::id::Id::MP3,
                _ => return Err(format!("Stream entry codec must be opus/vorbis/flac/mp3")),
            }
        } else if let ShoutFormat::MP3 = container {
            codec::id::Id::MP3
        } else {
            return Err(format!("Stream entry codec must be specified, or a default(for the mp3 \
                                container)"));
        };

        let bitrate = table.remove("bitrate")
                           .and_then(|value| value.as_integer())
                           .map(|i| i as usize);

        let mount = if let Some(value) = table.remove("mount") {
            match value.as_str() {
                Some(v) => v.to_owned(),
                None => return Err(format!("Mount point must be a string, {:?} is invalid", value)),
            }
        } else {
            return Err(format!("Stream entries must contain a mount field!"));
        };

        Ok(StreamConfig {
            container: container,
            codec: codec,
            bitrate: bitrate,
            mount: mount,
        })
    } else {
        Err(format!("Stream entries must be tables!"))
    }
}

fn parse_radio(input: toml::Table) -> Result<RadioConfig, String> {
    let table = try!(input.remove("radio")
                          .and_then(|v| v.as_table())
                          .map(|t| t.clone())
                          .ok_or(format!("Config must contain a valid radio table!")));

    let host = try!(table.remove("host")
                         .and_then(|v| v.as_str())
                         .map(|s| s.to_owned())
                         .ok_or(format!("Config must contain a radio table with a valid host \
                                         field")));
    let port = try!(table.remove("port")
                         .and_then(|v| v.as_integer())
                         .map(|i| i as u16)
                         .ok_or(format!("Config must contain a radio table with a valid port \
                                         field")));
    let user = try!(table.remove("user")
                         .and_then(|v| v.as_str())
                         .map(|s| s.to_owned())
                         .ok_or(format!("Config must contain a radio table with a valid user \
                                         field")));
    let password = try!(table.remove("password")
                         .and_then(|v| v.as_str())
                         .map(|s| s.to_owned())
                         .ok_or(format!("Config must contain a radio table with a valid password \
                                         field")));
    Ok(RadioConfig {
        host: host,
        port: port,
        user: user,
        password: password,
    })
}

fn parse_api(input: toml::Table) -> Result<ApiConfig, String> {
    let table = try!(input.remove("api")
                          .and_then(|v| v.as_table())
                          .map(|t| t.clone())
                          .ok_or(format!("Config must contain a valid api table!")));

    let remote_random = try!(table.remove("host")
                         .and_then(|v| v.as_str())
                         .map(|s| s.to_owned())
                         .ok_or(format!("Config must contain a radio table with a valid random song api \
                                         field")));
    let port = try!(table.remove("port")
                         .and_then(|v| v.as_integer())
                         .map(|i| i as u16)
                         .ok_or(format!("Config must contain a api table with a valid port \
                                         field")));
    Ok(ApiConfig {
        port: port,
        remote_random: remote_random,
    })
}
