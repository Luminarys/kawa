#[macro_use]
extern crate error_chain;
extern crate ffmpeg_sys as sys;
extern crate libc;

pub use sys::AVCodecID;

use std::ffi::{CString, CStr};
use std::io::{self, Read, Write};
use std::{slice, ptr, mem, time};
use libc::{c_char, c_int, c_void, uint8_t};

error_chain! {
    errors {
        FFmpeg(reason: &'static str, code: c_int) {
            description("ffmpeg error")
                display("{}, ffmpeg error: {}", reason, get_error(*code))
        }
        Allocation {
            description("Failed to allocate a necessary structure")
                display("Allocation failed(OOM)")
        }
    }
}

macro_rules! str_conv {
    ($s:expr) => {{
        let c_string = CString::new($s).unwrap();
        c_string.as_ptr()
    }}
}

macro_rules! ck_null {
    ($s:expr) => {
        if $s.is_null() {
            return Err(ErrorKind::Allocation.into());
        }
    }
}

const FFMPEG_BUFFER_SIZE: usize = 4096;

pub struct Graph {
    #[allow(dead_code)] // The graph needs to be kept as context for the filters
    graph: GraphP,
    #[allow(dead_code)]
    splitter: *mut sys::AVFilterContext, // We don't actually need this, but it's nice to have
    in_frame: *mut sys::AVFrame,
    out_frame: *mut sys::AVFrame,
    input: GraphInput,
    outputs: Vec<GraphOutput>,
}

pub struct GraphBuilder {
    graph: GraphP,
    input: GraphInput,
    outputs: Vec<GraphOutput>,
}

struct GraphOutput {
    output: Output,
    ctx: *mut sys::AVFilterContext,
}

struct GraphInput {
    input: Input,
    ctx: *mut sys::AVFilterContext,
}

pub struct Input {
    ctx: *mut sys::AVFormatContext,
    codec_ctx: *mut sys::AVCodecContext,
    stream: *mut sys::AVStream,
    _opaque: Opaque,
}

pub struct Output {
    ctx: *mut sys::AVFormatContext,
    codec_ctx: *mut sys::AVCodecContext,
    stream: *mut sys::AVStream,
    _opaque: Opaque,
    header_signal: fn(*mut c_void),
    packet_signal: fn(*mut c_void, f64),
    body_signal: fn(*mut c_void),
}

#[derive(Debug, Clone)]
pub struct Metadata {
    pub title: Option<String>,
    pub album: Option<String>,
    pub artist: Option<String>,
    pub genre: Option<String>,
    pub date: Option<String>,
    pub track: Option<String>,
}

struct Opaque {
    ptr: *mut c_void,
    cleanup: fn(*mut c_void),
}

struct GraphP {
    ptr: *mut sys::AVFilterGraph,
}

pub trait Sink : Write {
    fn header_written(&mut self) { }
    fn packet_written(&mut self, _: f64) { }
    fn body_written(&mut self) { }
}

impl Graph {
    pub fn run(mut self) -> Result<()> {
        unsafe {
            // Write header
            for o in self.outputs.iter() {
                match sys::avformat_write_header(o.output.ctx, ptr::null_mut()) {
                    0 => {
                        (o.output.header_signal)(o.output._opaque.ptr);
                    }
                    e => return Err(ErrorKind::FFmpeg("failed to write header", e).into()),
                }
            }

            let mut res = self.execute_tc();
            if res.is_ok() {
                res = self.try_flush();
                for o in self.outputs.iter() {
                    (o.output.body_signal)(o.output._opaque.ptr);
                    sys::av_write_trailer(o.output.ctx);
                }
            } else {
                for o in self.outputs.iter() {
                    (o.output.body_signal)(o.output._opaque.ptr);
                }

                if self.try_flush().is_err() {
                    // TODO: ?
                }

                for o in self.outputs.iter() {
                    sys::av_write_trailer(o.output.ctx);
                }
            }
            res
        }
    }

    unsafe fn execute_tc(&mut self) -> Result<()> {
        self.input.input.read_frames(self.in_frame, || {
            (*self.in_frame).pts = sys::av_frame_get_best_effort_timestamp(self.in_frame);
            let pres = self.process_frame(self.in_frame);
            sys::av_frame_unref(self.in_frame);
            pres
        })?;

        Ok(())
    }

    unsafe fn process_frame(&self, frame: *mut sys::AVFrame) -> Result<()> {
        // Push the frame into the graph source
        match sys::av_buffersrc_add_frame_flags(self.input.ctx, frame, sys::AV_BUFFERSRC_FLAG_KEEP_REF as i32) {
            0 => { }
            e => return Err(ErrorKind::FFmpeg("failed to add frame to graph source", e).into()),
        }

        // Pull out frames from each graph sink, sends them into the codecs for encoding, then
        // writes out the received packets
        for output in self.outputs.iter() {
            loop {
                match sys::av_buffersink_get_frame(output.ctx, self.out_frame) {
                    0 => { }
                    e if e == sys::AVERROR(libc::EAGAIN) => { break }
                    e if e == sys::AVERROR_EOF => { break }
                    e => return Err(ErrorKind::FFmpeg("failed to get frame from graph sink", e).into()),
                }

                { let r = output.output.write_frame(self.out_frame); sys::av_frame_unref(self.out_frame); r}?;
            }
            sys::av_frame_unref(self.out_frame);
        }
        Ok(())
    }

    unsafe fn try_flush(&self) -> Result<()> {
        let mut res = self.input.input.flush_frames(self.in_frame, || {
            (*self.in_frame).pts = sys::av_frame_get_best_effort_timestamp(self.in_frame);
            let pres = self.process_frame(self.in_frame);
            sys::av_frame_unref(self.in_frame);
            pres
        });

        // Flush everything
        res = res.and(self.process_frame(ptr::null_mut()));
        for o in self.outputs.iter() {
            // If the codec needs flushing, do so
            if ((*(*o.output.codec_ctx).codec).capabilities as u32 & sys::AV_CODEC_CAP_DELAY) != 0 {
                res = res.and(o.output.write_frame(ptr::null_mut()));
            }
        }
        res
    }
}

impl Drop for Graph {
    fn drop(&mut self) {
        unsafe {
            sys::av_frame_free(&mut self.in_frame);
            sys::av_frame_free(&mut self.out_frame);
        }
    }
}

unsafe impl Send for Graph { }

impl GraphBuilder {
    pub fn new(input: Input) -> Result<GraphBuilder> {
        unsafe {
            let graph = sys::avfilter_graph_alloc();
            ck_null!(graph);
            let buffersrc = sys::avfilter_get_by_name(str_conv!("abuffer"));
            ck_null!(buffersrc);
            let buffersrc_ctx = sys::avfilter_graph_alloc_filter(graph, buffersrc, str_conv!("in"));
            ck_null!(buffersrc_ctx);
            let time_base = (*input.stream).time_base;
            let sample_fmt = CStr::from_ptr(sys::av_get_sample_fmt_name((*input.codec_ctx).sample_fmt))
                .to_str().chain_err(|| "failed to parse format!")?;
            let args = format!("time_base={}/{}:sample_rate={}:sample_fmt={}:channel_layout=0x{:X}",
                               time_base.num, time_base.den, (*input.codec_ctx).sample_rate,
                               sample_fmt, (*input.codec_ctx).channel_layout);

            match sys::avfilter_init_str(buffersrc_ctx, str_conv!(&args[..])) {
                0 => { }
                e => return Err(ErrorKind::FFmpeg("failed to initialize buffersrc", e).into()),
            }

            Ok(GraphBuilder {
                input: GraphInput {
                    input,
                    ctx: buffersrc_ctx,
                },
                outputs: Vec::new(),
                graph: GraphP { ptr: graph },
            })
        }
    }

    pub fn add_output(&mut self, output: Output) -> Result<&mut Self> {
        let id = format!("out{}", self.outputs.len());
        unsafe {
            // Configure the encoder based on the decoder, then initialize it
            let ref input = self.input.input;
            if (*output.codec_ctx).codec_id == sys::AVCodecID::AV_CODEC_ID_OPUS {
                // OPUS only supports 48kHz sample rates
                (*output.codec_ctx).sample_rate = 48000;
            } else if (*output.codec_ctx).codec_id == sys::AVCodecID::AV_CODEC_ID_AAC {
                (*output.codec_ctx).sample_rate = 48000;
            } else if (*output.codec_ctx).codec_id == sys::AVCodecID::AV_CODEC_ID_MP3 {
                // MP3 can't handle 192 kHz, so encode at 44.1
                (*output.codec_ctx).sample_rate = 44100;
            } else {
                (*output.codec_ctx).sample_rate = (*input.codec_ctx).sample_rate;
            }
            if (*output.codec_ctx).bit_rate == 0 {
                (*output.codec_ctx).bit_rate = (*input.codec_ctx).bit_rate;
            }
            (*output.codec_ctx).channel_layout = (*input.codec_ctx).channel_layout;
            (*output.codec_ctx).channels = sys::av_get_channel_layout_nb_channels((*input.codec_ctx).channel_layout);
            let time_base = sys::AVRational {
                num: 1,
                den: (*output.codec_ctx).sample_rate,
            };
            (*output.codec_ctx).time_base = time_base;
            (*output.stream).time_base = time_base;

            sys::av_dict_copy(&mut (*output.ctx).metadata, (*self.input.input.ctx).metadata, 0);

            match sys::avcodec_open2(output.codec_ctx, (*output.codec_ctx).codec, ptr::null_mut()) {
                0 => { }
                e => return Err(ErrorKind::FFmpeg("failed to open audio decoder", e).into()),
            }
            match sys::avcodec_parameters_from_context((*output.stream).codecpar, output.codec_ctx) {
                0 => { },
                e => return Err(ErrorKind::FFmpeg("failed to configure output stream", e).into()),
            }

            // Create and configure the sink filter
            let buffersink = sys::avfilter_get_by_name(str_conv!("abuffersink"));
            ck_null!(buffersink);
            let buffersink_ctx = sys::avfilter_graph_alloc_filter(self.graph.ptr, buffersink, str_conv!(&id[..]));
            ck_null!(buffersink_ctx);

            match sys::av_opt_set_bin(buffersink_ctx as *mut c_void, str_conv!("sample_rates"), mem::transmute(&(*output.codec_ctx).sample_rate),
            mem::size_of_val(&(*output.codec_ctx).sample_rate) as c_int, sys::AV_OPT_SEARCH_CHILDREN) {
                0 => { }
                e => return Err(ErrorKind::FFmpeg("failed to configure buffersink sample_rates", e).into()),
            }
            match sys::av_opt_set_bin(buffersink_ctx as *mut c_void, str_conv!("sample_fmts"), mem::transmute(&(*output.codec_ctx).sample_fmt),
            mem::size_of_val(&(*output.codec_ctx).sample_fmt) as c_int, sys::AV_OPT_SEARCH_CHILDREN) {
                0 => { }
                e => return Err(ErrorKind::FFmpeg("failed to configure buffersink sample_fmts", e).into()),
            }
            match sys::av_opt_set_bin(buffersink_ctx as *mut c_void, str_conv!("channel_layouts"), mem::transmute(&(*output.codec_ctx).channel_layout),
            mem::size_of_val(&(*output.codec_ctx).channel_layout) as c_int, sys::AV_OPT_SEARCH_CHILDREN) {
                0 => { }
                e => return Err(ErrorKind::FFmpeg("failed to configure buffersink channel_layouts", e).into()),
            }
            match sys::avfilter_init_str(buffersink_ctx, ptr::null()) {
                0 => { }
                e => return Err(ErrorKind::FFmpeg("failed to initialize buffersink", e).into()),
            }
            self.outputs.push(GraphOutput {
                output,
                ctx: buffersink_ctx,
            });
        }
        Ok(self)
    }

    pub fn build(self) -> Result<Graph> {
        unsafe {
            // Create the audio split filter and wire it up
            let asplit = sys::avfilter_get_by_name(str_conv!("asplit"));
            ck_null!(asplit);
            let asplit_ctx = sys::avfilter_graph_alloc_filter(self.graph.ptr, asplit, str_conv!("splitter"));
            ck_null!(asplit_ctx);
            match sys::av_opt_set_int(asplit_ctx as *mut c_void, str_conv!("outputs"), self.outputs.len() as i64, sys::AV_OPT_SEARCH_CHILDREN) {
                0 => { }
                e => return Err(ErrorKind::FFmpeg("failed to configure asplit", e).into()),
            }
            match sys::avfilter_init_str(asplit_ctx, ptr::null()) {
                0 => { }
                e => return Err(ErrorKind::FFmpeg("failed to initialize asplit", e).into()),
            }
            match sys::avfilter_link(self.input.ctx, 0, asplit_ctx, 0) {
                0 => { }
                e => return Err(ErrorKind::FFmpeg("failed to link input to asplit", e).into()),
            }

            for (i, output) in self.outputs.iter().enumerate() {
                match sys::avfilter_link(asplit_ctx, i as u32, output.ctx, 0) {
                    0 => { }
                    e => return Err(ErrorKind::FFmpeg("failed to link output to asplit", e).into()),
                }
            }

            // validate the graph
            match sys::avfilter_graph_config(self.graph.ptr, ptr::null_mut()) {
                0 => { }
                e => return Err(ErrorKind::FFmpeg("failed to configure the filtergraph", e).into()),
            }

            // Align frame sizes on the buffersinks
            for o in self.outputs.iter() {
                sys::av_buffersink_set_frame_size(o.ctx, (*o.output.codec_ctx).frame_size as u32);
            }

            Ok(Graph {
                graph: self.graph,
                input: self.input,
                in_frame: sys::av_frame_alloc(),
                out_frame: sys::av_frame_alloc(),
                outputs: self.outputs,
                splitter: asplit_ctx,
            })
        }
    }
}

unsafe impl Send for GraphBuilder { }

impl Input {
    pub fn new<T: Read + Send + Sized>(t: T, container: &str) -> Result<Input> {
        unsafe {
            // Cache page size used here
            let buffer = sys::av_malloc(FFMPEG_BUFFER_SIZE) as *mut u8;
            ck_null!(buffer);
            let opaque = Opaque::new(t);
            let io_ctx = sys::avio_alloc_context(buffer, FFMPEG_BUFFER_SIZE as c_int, 0, opaque.ptr, Some(read_cb::<T>), None, None);
            ck_null!(io_ctx);

            let mut ps = sys::avformat_alloc_context();
            if ps.is_null() {
                return Err(ErrorKind::Allocation.into());
            }
            (*ps).pb = io_ctx;
            let format = sys::av_find_input_format(str_conv!(container));
            if format.is_null() {
                bail!("Could not derive format from container!");
            }
            let ctx = match sys::avformat_open_input(&mut ps, ptr::null(), format, ptr::null_mut()) {
                0 => ps,
                e => return Err(ErrorKind::FFmpeg("failed to open input context", e).into()),
            };
            match sys::avformat_find_stream_info(ctx, ptr::null_mut()) {
                0 => { },
                e => return Err(ErrorKind::FFmpeg("failed to get stream info", e).into()),
            }
            let mut codec = ptr::null_mut();
            let stream_idx = match sys::av_find_best_stream(ctx, sys::AVMediaType::AVMEDIA_TYPE_AUDIO, -1, -1, &mut codec, 0) {
                s if s >= 0 => s as usize,
                e => return Err(ErrorKind::FFmpeg("failed to get audio stream from input", e).into()),
            };
            if codec.is_null() {
                bail!("Failed to find a suitable codec!");
            }

            let codec_ctx = sys::avcodec_alloc_context3(codec);
            ck_null!(codec_ctx);
            let stream = *(*ctx).streams.offset(stream_idx as isize);
            match sys::av_opt_set_int(codec_ctx as *mut c_void, str_conv!("refcounted_frames"), 1, 0) {
                0 => { },
                e => return Err(ErrorKind::FFmpeg("failed to configure codec", e).into()),
            }
            match sys::avcodec_parameters_to_context(codec_ctx, (*stream).codecpar) {
                0 => { },
                e => return Err(ErrorKind::FFmpeg("failed to configure output stream", e).into()),
            }
            match sys::avcodec_open2(codec_ctx, codec, ptr::null_mut()) {
                0 => { }
                e => return Err(ErrorKind::FFmpeg("failed to open audio decoder", e).into()),
            }

            Ok(Input {
                ctx,
                codec_ctx,
                stream,
                _opaque: opaque,
            })
        }
    }

    pub fn duration(&self) -> time::Duration {
        unsafe {
            let s = sys::av_q2d((*self.stream).time_base);
            let dur = s * (*self.stream).duration as f64;
            time::Duration::from_millis((dur * 1000.) as u64)
        }
    }

    pub fn metadata(&self) -> Metadata {
        unsafe {
            Metadata {
                title: self.get_metadata_val("title"),
                album: self.get_metadata_val("album"),
                artist: self.get_metadata_val("artist"),
                genre: self.get_metadata_val("genre"),
                date: self.get_metadata_val("date"),
                track: self.get_metadata_val("track"),
            }
        }
    }

    unsafe fn get_metadata_val(&self, opt: &str) -> Option<String> {
        let entry = sys::av_dict_get((*self.ctx).metadata, str_conv!(opt), ptr::null(), 0);
        if entry.is_null() {
            None
        } else {
            let len = libc::strlen((*entry).value) + 1;
            let mut val = vec![0u8; len];
            let mptr = val.as_mut_ptr() as *mut c_char;
            libc::strcpy(mptr, (*entry).value as *const c_char);
            val.pop();
            String::from_utf8(val).ok()
        }
    }

    unsafe fn read_frames<F: FnMut() -> Result<()>>(&self, frame: *mut sys::AVFrame, mut f: F) -> Result<()> {
        let mut packet: sys::AVPacket = mem::uninitialized();
        packet.data = ptr::null_mut();
        packet.size = 0;

        'outer: loop {
            loop {
                match sys::av_read_frame(self.ctx, &mut packet) {
                    0 => { }
                    e if e == sys::AVERROR_EOF => { break 'outer; }
                    e  => { return Err(ErrorKind::FFmpeg("failed to read frame", e).into()); }
                }
                let stream_idx = (&packet).stream_index as isize;
                let stream = *(*self.ctx).streams.offset(stream_idx);
                if stream == self.stream {
                    break;
                } else {
                    sys::av_packet_unref(&mut packet);
                }
            }

            match { let r = sys::avcodec_send_packet(self.codec_ctx, &packet); sys::av_packet_unref(&mut packet); r} {
                0 => { }
                e if e == sys::AVERROR_EOF => { break 'outer; }
                e  => { return Err(ErrorKind::FFmpeg("failed to decode packet", e).into()); }
            }

            loop {
                // Try to get a frame, if not try to read packets and decode them
                match sys::avcodec_receive_frame(self.codec_ctx, frame) {
                    0 => { f()?; },
                    e if e == sys::AVERROR(libc::EAGAIN) => { break; }
                    e if e == sys::AVERROR_EOF => { break 'outer; }
                    e => { return Err(ErrorKind::FFmpeg("failed to receive frame", e).into()); }
                }
            }
        }

        Ok(())
    }

    unsafe fn flush_frames<F: FnMut() -> Result<()>>(&self, frame: *mut sys::AVFrame, mut f: F) -> Result<()> {
        if ((*(*self.codec_ctx).codec).capabilities as u32 & sys::AV_CODEC_CAP_DELAY) == 0 {
            return Ok(());
        }

        let mut packet: sys::AVPacket = mem::uninitialized();
        packet.data = ptr::null_mut();
        packet.size = 0;

        let mut res = match sys::avcodec_send_packet(self.codec_ctx, &packet) {
            0 => Ok(()),
            e => Err(ErrorKind::FFmpeg("failed to handle EOF packet", e).into()),
        };

        loop {
            // Try to get a frame, if not try to read packets and decode them
            let r = match sys::avcodec_receive_frame(self.codec_ctx, frame) {
                0 => f(),
                e if e == sys::AVERROR(libc::EAGAIN) => break,
                e if e == sys::AVERROR_EOF => break,
                e => Err(ErrorKind::FFmpeg("failed to receive frame", e).into()),
            };
            res = res.and(r);
        }
        res
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        unsafe {
            sys::av_free((*(*self.ctx).pb).buffer as *mut c_void);
            sys::av_free((*self.ctx).pb as *mut c_void);
            sys::avformat_close_input(&mut self.ctx);
            sys::avcodec_free_context(&mut self.codec_ctx);
        }
    }
}

impl Output {
    pub fn new_writer<T: Write + Send + Sized>(t: T, container: &str, codec_id: sys::AVCodecID, bit_rate: Option<i64>) -> Result<Output> {
        struct SW<T: Write>(T);
        impl<T: Write> Write for SW<T> {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> { self.0.write(buf) }
            fn flush(&mut self) -> io::Result<()> { self.0.flush() }
        }
        impl<T: Write> Sink for SW<T> { };
        Output::new(SW(t), container, codec_id, bit_rate)
    }

    pub fn new<T: Sink + Send + Sized>(t: T, container: &str, codec_id: sys::AVCodecID, bit_rate: Option<i64>) -> Result<Output> {
        unsafe {
            let buffer = sys::av_malloc(4096) as *mut u8;
            ck_null!(buffer);
            let opaque = Opaque::new(t);
            let io_ctx = sys::avio_alloc_context(buffer, 4096, 1, opaque.ptr, None, Some(write_cb::<T>), None);
            ck_null!(io_ctx);

            let mut ctx = ptr::null_mut();
            match sys::avformat_alloc_output_context2(&mut ctx, ptr::null_mut(), str_conv!(container), ptr::null()) {
                0 => { },
                e => return Err(ErrorKind::FFmpeg("failed to open output context", e).into()),
            };
            (*ctx).pb = io_ctx;

            let codec = sys::avcodec_find_encoder(codec_id);
            if codec.is_null() {
                bail!("invalid codec provided!");
            }
            let codec_ctx = sys::avcodec_alloc_context3(codec);
            ck_null!(codec_ctx);
            if let Some(br) = bit_rate {
                (*codec_ctx).bit_rate = br as i64 * 1000;
            } else {
                (*codec_ctx).bit_rate = 0;
            }
            (*codec_ctx).sample_fmt = *(*codec).sample_fmts;
            if container == "ogg" {
                // Set page size to a small duration(0.05s), to minimize skip loss
                sys::av_opt_set_int((*ctx).priv_data as *mut c_void, str_conv!("page_duration"), 50000, 0);
            }
            let stream = sys::avformat_new_stream(ctx, codec);
            ck_null!(stream);

            Ok(Output {
                ctx,
                _opaque: opaque,
                codec_ctx,
                stream,
                header_signal: sink_header_written::<T>,
                packet_signal: sink_packet_written::<T>,
                body_signal: sink_body_written::<T>,
            })
        }
    }

    unsafe fn write_frame(&self, frame: *mut sys::AVFrame) -> Result<()> {
        let mut out_pkt: sys::AVPacket = mem::uninitialized();
        out_pkt.data = ptr::null_mut();
        out_pkt.size = 0;
        sys::av_init_packet(&mut out_pkt);
        match sys::avcodec_send_frame(self.codec_ctx, frame) {
            0 => { }
            e => return Err(ErrorKind::FFmpeg("failed to send frame to encoder", e).into()),
        }
        loop {
            match sys::avcodec_receive_packet(self.codec_ctx, &mut out_pkt) {
                0 => { }
                e if e == sys::AVERROR(libc::EAGAIN) => { break }
                e if e == sys::AVERROR_EOF => { break }
                e => return Err(ErrorKind::FFmpeg("failed to get packet from encoder", e).into()),
            }

            sys::av_packet_rescale_ts(&mut out_pkt, (*self.codec_ctx).time_base, (*self.stream).time_base);
            out_pkt.stream_index = 0;
            let s = sys::av_q2d((*self.stream).time_base);
            let pts = s * out_pkt.pts as f64;

            match { let r = sys::av_write_frame(self.ctx, &mut out_pkt); sys::av_packet_unref(&mut out_pkt); r } {
                0 => { }
                e => return Err(ErrorKind::FFmpeg("failed to write packet", e).into()),
            }
            sys::avio_flush((*self.ctx).pb);
            (self.packet_signal)(self._opaque.ptr, pts);
            sys::av_packet_unref(&mut out_pkt);
        }
        Ok(())
    }

    unsafe fn flush_queue(&self) {
        let mut out_pkt: sys::AVPacket = mem::uninitialized();
        out_pkt.data = ptr::null_mut();
        out_pkt.size = 0;
        sys::av_init_packet(&mut out_pkt);
        sys::avcodec_send_frame(self.codec_ctx, ptr::null());
        loop {
            match sys::avcodec_receive_packet(self.codec_ctx, &mut out_pkt) {
                0 => { }
                _ => break
            }
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        unsafe {
            self.flush_queue();
            sys::av_free((*(*self.ctx).pb).buffer as *mut c_void);
            sys::av_free((*self.ctx).pb as *mut c_void);
            sys::avformat_free_context(self.ctx);
            sys::avcodec_free_context(&mut self.codec_ctx);
        }
    }
}

unsafe extern fn read_cb<T: Read + Sized>(opaque: *mut c_void, buf: *mut uint8_t, len: c_int) -> c_int {
    let reader = &mut *(opaque as *mut T);
    let s = slice::from_raw_parts_mut(buf, len as usize);
    match reader.read(s) {
        Ok(a) => a as c_int,
        Err(e) => {
            if e.kind() == io::ErrorKind::WouldBlock {
                0
            } else {
                sys::AVERROR_EXIT
            }
        }
    }
}

unsafe extern fn write_cb<T: Sink + Sized>(opaque: *mut c_void, buf: *mut uint8_t, len: c_int) -> c_int {
    let writer = &mut *(opaque as *mut T);
    let s = slice::from_raw_parts(buf, len as usize);
    match writer.write(s) {
        Ok(a) => a as c_int,
        Err(e) => {
            if e.kind() == io::ErrorKind::WouldBlock {
                0
            } else {
                sys::AVERROR_EXIT
            }
        }
    }
}

fn sink_header_written<T: Sink + Sized>(opaque: *mut c_void) {
    unsafe {
        let s = &mut *(opaque as *mut T);
        s.header_written();
    }
}

fn sink_packet_written<T: Sink + Sized>(opaque: *mut c_void, pts: f64) {
    unsafe {
        let s = &mut *(opaque as *mut T);
        s.packet_written(pts);
    }
}

fn sink_body_written<T: Sink + Sized>(opaque: *mut c_void) {
    unsafe {
        let s = &mut *(opaque as *mut T);
        s.body_written();
    }
}

impl Drop for GraphP {
    fn drop(&mut self) {
        unsafe {
            sys::avfilter_graph_free(&mut self.ptr);
        }
    }
}

impl Opaque {
    fn new<T: Sized>(t: T) -> Opaque {
        Opaque {
            ptr: Box::into_raw(Box::new(t)) as *mut c_void,
            cleanup: cleanup_opaque::<T>,
        }
    }
}

impl Drop for Opaque {
    fn drop(&mut self) {
        (self.cleanup)(self.ptr);
    }
}

fn cleanup_opaque<T: Sized>(opaque: *mut c_void) {
    unsafe {
        let b = Box::from_raw(opaque as *mut T);
        drop(b);
    }
}

fn get_error(code: c_int) -> String {
    let len = 200;
    let mut raw = vec![0u8; len];
    unsafe {
        // To satisfy the CString invariant(buffer ending with a \0 and containing a single \0),
        // we create a large buffer to get the error string, then copy in the exact size later
        let ptr = raw.as_mut_ptr() as *mut c_char;
        sys::av_strerror(code, ptr, len);
        let len = libc::strlen(ptr) + 1;

        let mut msg = vec![0u8; len];
        let mptr = msg.as_mut_ptr() as *mut c_char;
        libc::strcpy(mptr, ptr);
        // Pop the null byte
        msg.pop();
        String::from_utf8(msg).unwrap_or("improper error".to_owned())
    }
}

pub fn init() {
    unsafe {
        sys::av_register_all();
        sys::avfilter_register_all();
    }
}

#[cfg(test)]
mod tests {
    use super::{GraphBuilder, Input, Output, init, Result};
    use std::fs::File;

    #[test]
    fn test_instantiate_input() {
        init();
        let f = File::open("test/test.mp3").unwrap();
        Input::new(f, "mp3").unwrap();
    }

    #[test]
    fn test_instantiate_output() {
        init();
        let d = vec![0u8; 1024 * 16];
        Output::new_writer(d, "ogg", super::sys::AVCodecID::AV_CODEC_ID_VORBIS, None).unwrap();
    }

    #[test]
    fn test_run_graph() {
        init();
        run_graph().unwrap();
    }

    fn run_graph() -> Result<()> {
        let fin = File::open("test/test.mp3").unwrap();
        let fout1 = File::create("test/test.ogg").unwrap();
        let fout2 = File::create("test/test2.ogg").unwrap();
        let fout3 = File::create("test/test3.ogg").unwrap();
        let fout4 = File::create("test/test4.mp3").unwrap();
        let fout5 = File::create("test/test5.aac").unwrap();

        let i = Input::new(fin, "mp3")?;
        let o1 = Output::new_writer(fout1, "ogg", super::AVCodecID::AV_CODEC_ID_OPUS, None)?;
        let o2 = Output::new_writer(fout2, "ogg", super::AVCodecID::AV_CODEC_ID_VORBIS, None)?;
        let o3 = Output::new_writer(fout3, "ogg", super::AVCodecID::AV_CODEC_ID_FLAC, None)?;
        let o4 = Output::new_writer(fout4, "mp3", super::AVCodecID::AV_CODEC_ID_MP3, None)?;
        let o4 = Output::new_writer(fout5, "adts", super::AVCodecID::AV_CODEC_ID_AAC, None)?;
        let mut gb = GraphBuilder::new(i)?;
        gb.add_output(o1)?;
        gb.add_output(o2)?;
        gb.add_output(o3)?;
        gb.add_output(o4)?;
        gb.add_output(o5)?;
        gb.build()?.run()
    }

    #[test]
    fn test_metadata() {
        init();
        let fin = File::open("/tmp/in.flac").unwrap();
        let i = Input::new(fin, "flac").unwrap();
        let md = i.metadata();
        println!("{:?}", md);
        assert!(md.title.is_some());
    }
}
