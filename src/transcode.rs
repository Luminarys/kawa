use libc::{c_int, uint8_t, c_void, int64_t};
use ffmpeg::{self, format, codec, media, frame, filter};
use std::sync::{Arc, Mutex};
use std::thread;
use std::io::Write;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};


pub fn transcode(in_data: Arc<Vec<u8>>,
                 ict: &str,
                 out_data: Arc<Mutex<Vec<u8>>>,
                 oct: &str,
                 format: codec::id::Id,
                 bitrate: Option<usize>,
                 cancel: Arc<AtomicBool>)
                 -> Result<thread::JoinHandle<()>, ffmpeg::Error> {
    let filter = "anull".to_owned();

    let io_ictx = format::io::Context::new(4096,
                                           false,
                                           (0usize, in_data),
                                           Some(read_packet),
                                           None,
                                           Some(seek_packet));
    let mut ictx = format::open_custom_io(io_ictx, true, ict).unwrap().input();

    let io_octx = format::io::Context::new(4096,
                                           true,
                                           (out_data.clone(), cancel.clone()),
                                           None,
                                           Some(write_packet),
                                           None);
    let mut octx = format::open_custom_io(io_octx, false, oct).unwrap().output();

    let mut transcoder = transcoder(&mut ictx, &mut octx, format, &filter, bitrate).unwrap();
    let oct = oct.to_owned();
    let tok = thread::spawn(move || {
        octx.set_metadata(ictx.metadata().to_owned());
        octx.write_header().unwrap();

        let in_time_base = transcoder.decoder.time_base();
        let out_time_base = octx.stream(0).unwrap().time_base();

        let mut decoded = frame::Audio::empty();
        let mut encoded = ffmpeg::Packet::empty();
        let mut err = false;
        'outer: for (stream, mut packet) in ictx.packets() {
            if stream.index() == transcoder.stream {
                packet.rescale_ts(stream.time_base(), in_time_base);

                if let Ok(true) = transcoder.decoder.decode(&packet, &mut decoded) {
                    let timestamp = decoded.timestamp();
                    decoded.set_pts(timestamp);

                    transcoder.filter.get("in").unwrap().source().add(&decoded).unwrap();

                    while let Ok(..) = transcoder.filter
                                                 .get("out")
                                                 .unwrap()
                                                 .sink()
                                                 .frame(&mut decoded) {
                        if let Ok(true) = transcoder.encoder.encode(&decoded, &mut encoded) {
                            encoded.set_stream(0);
                            encoded.rescale_ts(in_time_base, out_time_base);
                            if let Err(_) = encoded.write_interleaved(&mut octx) {
                                err = true;
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }

        if !err {
            transcoder.filter.get("in").unwrap().source().flush().unwrap();

            while let Ok(..) = transcoder.filter.get("out").unwrap().sink().frame(&mut decoded) {
                if let Ok(true) = transcoder.encoder.encode(&decoded, &mut encoded) {
                    encoded.set_stream(0);
                    encoded.rescale_ts(in_time_base, out_time_base);
                    if let Err(_) = encoded.write_interleaved(&mut octx) {
                        err = true;
                        break;
                    }
                }
            }
        }
        if !err {
            if let Ok(true) = transcoder.encoder.flush(&mut encoded) {
                encoded.set_stream(0);
                encoded.rescale_ts(in_time_base, out_time_base);
                if let Err(_) = encoded.write_interleaved(&mut octx) {
                }
            }

        }
        if let Ok(()) = octx.write_trailer() {
        };

        println!("Transcoding over for oct {:?}", oct);
    });
    Ok(tok)
}

struct Transcoder {
    stream: usize,
    filter: filter::Graph,
    decoder: codec::decoder::Audio,
    encoder: codec::encoder::Audio,
}

fn transcoder(ictx: &mut format::context::Input,
              octx: &mut format::context::Output,
              codec: codec::id::Id,
              filter_spec: &str,
              bitrate: Option<usize>)
              -> Result<Transcoder, ffmpeg::Error> {
    let input = ictx.streams().best(media::Type::Audio).expect("could not find best audio stream");
    let decoder = try!(input.codec().decoder().audio());
    let codec = try!(ffmpeg::encoder::find(codec).unwrap().audio());
    let global = octx.format().flags().contains(ffmpeg::format::flag::GLOBAL_HEADER);

    let mut output = try!(octx.add_stream(codec));
    let mut encoder = try!(output.codec().encoder().audio());

    let channel_layout = codec.channel_layouts()
                              .map(|cls| cls.best(decoder.channel_layout().channels()))
                              .unwrap_or(ffmpeg::channel_layout::STEREO);

    if global {
        encoder.set_flags(ffmpeg::codec::flag::GLOBAL_HEADER);
    }

    // OPUS apparently only likes 48000, so this is probably fine
    encoder.set_rate(48000);
    encoder.set_channel_layout(channel_layout);
    encoder.set_channels(channel_layout.channels());
    encoder.set_format(codec.formats().expect("unknown supported formats").next().unwrap());
    if let Some(br) = bitrate {
        encoder.set_bit_rate(br * 1000);
        encoder.set_max_bit_rate(br * 1000);
    } else {
        encoder.set_bit_rate(decoder.bit_rate());
        encoder.set_max_bit_rate(decoder.max_bit_rate());
    }

    encoder.set_time_base((1, decoder.rate() as i32));
    output.set_time_base((1, decoder.rate() as i32));

    let encoder = try!(encoder.open_as(codec));
    let filter = try!(filter(filter_spec, &decoder, &encoder));

    Ok(Transcoder {
        stream: input.index(),
        filter: filter,
        decoder: decoder,
        encoder: encoder,
    })
}

macro_rules! rw_callback {
    ($name:ident, $func:ident, $t:ty) => {
        extern fn $name(opaque: *mut c_void, buffer: *mut uint8_t, buffer_len: c_int) -> c_int {
            unsafe {
                let output: &mut $t = &mut *(opaque as *mut $t);
                let buffer = slice::from_raw_parts_mut(buffer, buffer_len as usize);
                $func(output, buffer) as c_int
            }
        }
    };
    (seek, $name:ident, $func:ident, $t:ty) => {
        extern fn $name(opaque: *mut c_void, offset: int64_t, whence: c_int) -> int64_t {
            unsafe {
                let output: &mut $t = &mut *(opaque as *mut $t);
                $func(output, offset, whence) as int64_t
            }
        }
    };
}

fn write_to_buf(&(ref output, ref cancel): &(Arc<Mutex<Vec<u8>>>, Arc<AtomicBool>),
                buffer: &[u8])
                -> i32 {
    let mut data = output.lock().unwrap();
    if cancel.load(Ordering::SeqCst) {
        data.clear();
        return ffmpeg::sys::AVERROR_EXIT;
    }
    data.write(buffer).unwrap() as i32
}

rw_callback!(write_packet, write_to_buf, (Arc<Mutex<Vec<u8>>>, Arc<AtomicBool>));

fn read_buf(&mut (ref mut pos, ref input): &mut (usize, Arc<Vec<u8>>),
            mut buffer: &mut [u8])
            -> i32 {
    let len = buffer.len();
    if *pos + len < input.len() {
        let res = buffer.write(&input[*pos..*pos + len]).unwrap();
        *pos += len;
        res as i32
    } else if *pos < input.len() {
        buffer.write(&input[*pos..input.len()]).unwrap();
        ffmpeg::sys::AVERROR_EOF
    } else {
        ffmpeg::sys::AVERROR_EOF
    }
}

rw_callback!(read_packet, read_buf, (usize, Arc<Vec<u8>>));

fn seek_buf(&mut (ref mut pos, ref input): &mut (usize, Arc<Vec<u8>>),
            offset: i64,
            whence: i32)
            -> i64 {
    if whence == ffmpeg::sys::AVSEEK_SIZE {
        return input.len() as i64;
    }
    *pos = offset as usize;
    return offset;
}

rw_callback!(seek, seek_packet, seek_buf, (usize, Arc<Vec<u8>>));

fn filter(spec: &str,
          decoder: &codec::decoder::Audio,
          encoder: &codec::encoder::Audio)
          -> Result<filter::Graph, ffmpeg::Error> {
    let mut filter = filter::Graph::new();

    let args = format!("time_base={}:sample_rate={}:sample_fmt={}:channel_layout=0x{:x}",
                       decoder.time_base(),
                       decoder.rate(),
                       decoder.format().name(),
                       decoder.channel_layout().bits());

    try!(filter.add(&filter::find("abuffer").unwrap(), "in", &args));
    try!(filter.add(&filter::find("abuffersink").unwrap(), "out", ""));

    {
        let mut out = filter.get("out").unwrap();

        out.set_sample_format(encoder.format());
        out.set_channel_layout(encoder.channel_layout());
        out.set_sample_rate(encoder.rate());
    }

    try!(try!(try!(filter.output("in", 0)).input("out", 0)).parse(spec));
    try!(filter.validate());

    if let Some(codec) = encoder.codec() {
        if !codec.capabilities().contains(ffmpeg::codec::capabilities::VARIABLE_FRAME_SIZE) {
            filter.get("out").unwrap().sink().set_frame_size(encoder.frame_size());
        }
    }

    Ok(filter)
}
