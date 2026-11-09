fn main() {
    // rpath so the binary finds the vendored libfreerdp built by freerdp-sys
    if let Ok(dir) = std::env::var("DEP_FREERDP3_LIBDIR") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
}
