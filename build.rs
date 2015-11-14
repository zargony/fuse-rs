extern crate pkg_config;

use std::env;

fn show_libfuse_msg(lib : &str) {
	match pkg_config::find_library(lib) {
		Err(_) => panic!("libfuse is not installed. For OSX use `osxfuse`, for linux use `libfuse-dev` package."),
		Ok(_) => {}, 
	}
}

fn main () {
    let target = env::var("TARGET").unwrap();
    if target.ends_with("-apple-darwin") {
        // Use libosxfuse on OS X
        show_libfuse_msg("osxfuse"); 
    } else if target.ends_with("-unknown-linux-gnu") || target.ends_with("-unknown-freebsd") {
        // Use libfuse on Linux and FreeBSD
        show_libfuse_msg("fuse"); 
    } else {
        // Fail on unsupported platforms (e.g. Windows)
        panic!("Unsupported target platform");
    }
}
