use std::env;
use cntr_fuse::Filesystem;

struct NullFS;

impl Filesystem for NullFS {}

fn main() {
    env_logger::init();
    let mountpoint = env::args_os().nth(1).unwrap();
    cntr_fuse::mount(NullFS, mountpoint, &[]).unwrap();
}
