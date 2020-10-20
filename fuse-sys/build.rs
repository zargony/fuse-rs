use std::env;

#[cfg(not(target_os = "macos"))]
const LIBFUSE_NAME: &str = "fuse";

#[cfg(target_os = "macos")]
const LIBFUSE_NAME: &str = "osxfuse";

fn main() {
    let k = "FUSE_SYS_DELEGATE_LINKING";

    println!("cargo:rerun-if-env-changed={}", k);
    if env::var(k).is_ok() {
        return;
    }

    pkg_config::Config::new()
        .atleast_version("2.6.0")
        .probe(LIBFUSE_NAME)
        .map_err(|e| eprintln!("{}", e))
        .unwrap();
}
