# Rust FUSE - Filesystem in Userspace

![Crates.io](https://img.shields.io/crates/l/fuse)
[![Crates.io](https://img.shields.io/crates/v/fuse)](https://crates.io/crates/fuse)

## About

**fuse-rs** is a [Rust] library crate for easy implementation of [FUSE filesystems][libfuse] in userspace.

fuse-rs does not just provide bindings, it is a rewrite of the original FUSE C library to fully take advantage of Rust's architecture.

## Documentation

[Crate documentation][documentation]

## Details

A working FUSE filesystem consists of three parts:

1. The **kernel driver** that registers as a filesystem and forwards operations into a communication channel to a userspace process that handles them.
1. The **userspace library** (libfuse) that helps the userspace process to establish and run communication with the kernel driver.
1. The **userspace implementation** that actually processes the filesystem operations.

The kernel driver is provided by the FUSE project, the userspace implementation needs to be provided by the developer. fuse-rs provides a replacement for the libfuse userspace library between these two. This way, a developer can fully take advantage of the Rust type interface and runtime features when building a FUSE filesystem in Rust.

Except for a single setup (mount) function call and a final teardown (unmount) function call to libfuse, everything runs in Rust.

## Dependencies

FUSE must be installed to build or run programs that use fuse-rs (i.e. kernel driver and libraries. Some platforms may also require userland utils like `fusermount`). A default installation of FUSE is usually sufficient.

To build fuse-rs or any program that depends on it, `pkg-config` needs to be installed as well.

### Linux

[FUSE for Linux][libfuse] is available in most Linux distributions and usually called `fuse`. To install on a Debian based system:

```sh
sudo apt-get install fuse
```

Install on CentOS:

```sh
sudo yum install fuse
```

To build, FUSE libraries and headers are required. The package is usually called `libfuse-dev` or `fuse-devel`. Also `pkg-config` is required for locating libraries and headers.

```sh
sudo apt-get install libfuse-dev pkg-config
```

```sh
sudo yum install fuse-devel pkgconfig
```

### macOS

Installer packages can be downloaded from the [FUSE for macOS homepage][FUSE for macOS].

To install using [Homebrew]:

```sh
brew cask install macfuse
```

To install `pkg-config` (required for building only):

```sh
brew install pkg-config
```

### FreeBSD

Install packages `fusefs-libs` and `pkgconf`.

```sh
pkg install fusefs-libs pkgconf
```

## Usage

Put this in your `Cargo.toml`:

```toml
[dependencies]
fuse = "0.4"
```

To create a new filesystem, implement the trait `fuse::Filesystem`. See the [documentation] for details or the `examples` directory for some basic examples.

## To Do

There's still a lot of stuff to be done. Feel free to contribute. See the [list of issues][issues] on GitHub and search the source files for comments containing "`TODO`" or "`FIXME`" to see what's still missing.

## Compatibility

Developed and tested on macOS. Tested under [Linux][libfuse], [macOS][FUSE for macOS] and [FreeBSD][FUSEFS] using stable, beta and nightly [Rust] versions (see [CI] for details).

## Contribution

Fork, hack, submit pull request. Make sure to make it useful for the target audience, keep the project's philosophy and Rust coding standards in mind. For larger or essential changes, you may want to open an issue for discussion first. Also remember to update the [Changelog] if your changes are relevant to the users.

[issues]: https://github.com/zargony/fuse-rs/issues
[documentation]: https://docs.rs/fuse
[CI]: https://github.com/zargony/fuse-rs/actions

[Rust]: https://rust-lang.org
[Homebrew]: https://brew.sh
[Changelog]: https://keepachangelog.com/en/1.0.0/

[libfuse]: https://github.com/libfuse/libfuse/
[FUSE for macOS]: https://osxfuse.github.io
[FUSEFS]: https://wiki.freebsd.org/FUSEFS
