fn main() {
    println!("cargo:rustc-link-lib=static=z");

    println!("cargo:rustc-link-lib=static=mp3lame");
    println!("cargo:rustc-link-lib=static=opus");
    println!("cargo:rustc-link-lib=static=ogg");
    println!("cargo:rustc-link-lib=static=vorbis");
    println!("cargo:rustc-link-lib=static=vorbisfile");
    println!("cargo:rustc-link-lib=static=vorbisenc");

    println!("cargo:rustc-link-lib=static=avcodec");
    println!("cargo:rustc-link-lib=static=avutil");
    println!("cargo:rustc-link-lib=static=avformat");
    println!("cargo:rustc-link-lib=static=avfilter");
    println!("cargo:rustc-link-lib=static=avdevice");
    println!("cargo:rustc-link-lib=static=swresample");
    println!("cargo:rustc-link-lib=static=swscale");
}
