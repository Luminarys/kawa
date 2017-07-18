fn main() {
    println!("cargo:rustc-link-lib=avutil");
    println!("cargo:rustc-link-search=/usr/lib");
}

