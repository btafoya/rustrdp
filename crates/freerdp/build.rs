fn main() {
    // rpath so tests/examples find the vendored libfreerdp
    if let Ok(dir) = std::env::var("DEP_FREERDP3_LIBDIR") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
}
