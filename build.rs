extern crate pkg_config;

use std::env;

fn main () {
    let target = env::var("TARGET").unwrap();
    if target.ends_with("-apple-darwin") {
        // Use libosxfuse on OS X
        pkg_config::find_library("osxfuse").unwrap();
    } else if target.ends_with("-unknown-linux-gnu") || target.ends_with("-unknown-freebsd") {
        // Use libfuse on Linux and FreeBSD
        pkg_config::find_library("fuse").unwrap();
    } else {
        // Fail on unsupported platforms (e.g. Windows)
        panic!("Unsupported target platform");
    }
}
