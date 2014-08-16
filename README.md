# Rust FUSE - Filesystem in Userspace

[![Build Status](https://travis-ci.org/suhr/rust-fuse.svg?branch=master)](https://travis-ci.org/suhr/rust-fuse)

## About
[Rust](http://rust-lang.org/) library for easy implementation of [FUSE](http://osxfuse.github.io) filesystems in userspace.

This library does not just provide bindings, it is actually an improved rewrite of the original FUSE C library to fully take advantage of Rust's architecture.

## Details

A working FUSE filesystem consists of three parts:

1. The kernel driver that registers as a filesystem and forwards operations into a communication channel to a userspace process that handles them.
1. The userspace library (libfuse) that helps the userspace process to establish and run communication with the kernel driver.
1. The userspace implementation that actually processes the filesystem operations.

The kernel driver is provided by the FUSE project, the userspace implementation needs to be provided by the developer. This Rust library provides a replacement for the libfuse userspace library between these two. This way, a developer can fully take advantage of the Rust type interface and runtime features when building a FUSE filesystem in Rust.

Except for a single setup (mount) function call and a final teardown (umount) function call to libfuse, everything runs in Rust.

## How to

To create a new filesystem, implement the trait `Filesystem`. Filesystem operations from the kernel are dispatched to the methods of the `Filesystem` trait. Most methods get a `reply` parameter that must be used to eventually answer the request. All methods have default implementations that reply with neutral answers, so if you implement no method at all, you still get a mountable filesystem that does nothing.

To actually mount the filesystem, pass an object that implements `Filesystem` and the path of an (existing) mountpoint to the `mount` function. `mount` will not return until the filesystem is unmounted.

To mount a filesystem and keep running other code, use `spawn_mount` instead of `mount`. `spawn_mount` spawns a background task to handle filesystem operations while the filesystem is mounted. It returns a handle that should be stored to reference the mounted filesystem. If the handle is dropped, the filesystem is unmounted.

To unmount a filesystem, use any arbitrary unmount/eject method of your OS.

See the examples directory for some basic examples.

## To Do

There's still a lot of stuff to be done. Feel free to contribute.

- Interrupting a filesystem operation isn't handled yet.
- An additional more high level API would be nice. It should provide pathnames instead inode numbers and automatically handle concurrency and interruption (like the FUSE C library's high level API).

In general, search for "TODO" or "FIXME" in the source files to see what's still missing.

## Compatibility

Developed and tested on a Mac with [OSXFUSE](http://osxfuse.github.io) and [FUSE on Linux](http://fuse.sourceforge.net). It should (maybe with minor adjustments) also work with [FUSE on FreeBSD](https://wiki.freebsd.org/FuseFilesystem).
