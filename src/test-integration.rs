#[crate_type="test"];
#[feature(macro_rules)];

extern mod extra;

use std::cast::transmute;
use std::comm::{Chan};
use std::c_str::CString;
use std::io::timer::{Timer, sleep};
use std::io::{File, io_error, IoError, OtherIoError};
use std::io::fs::stat;
use std::io::process::{Process, ProcessConfig, ProcessExit, Ignored, InheritFd, MustDieSignal};
use std::iter::range_inclusive;
use std::path::Path;
use std::os::args;
use extra::tempfile::TempDir;
use std::str::from_utf8;
use std::libc::c_char;
use std::libc::funcs::posix88::signal::kill;
use std::task::spawn;

// Still need to copy the select! macro from std::comm--eventually it will be in the rust std lib, but not yet.
macro_rules! select {
    (
        $name1:pat = $port1:ident.$meth1:ident() => $code1:expr,
        $($name:pat = $port:ident.$meth:ident() => $code:expr),*
    ) => ({
        use std::comm::Select;
        let sel = Select::new();
        let mut $port1 = sel.add(&mut $port1);
        $( let mut $port = sel.add(&mut $port); )*
        let ret = sel.wait();
        if ret == $port1.id { let $name1 = $port1.$meth1(); $code1 }
        $( else if ret == $port.id { let $name = $port.$meth(); $code } )*
        else { unreachable!() }
    })
}

// Buf is a buffer that was just filled with a null terminated C string at its beginning.
// Construct a reference to a str of that string.
fn c_buf_as_str<'a>(buf:&'a [i8]) -> &'a str {
	unsafe {
		transmute(CString::new(buf.as_ptr(), false).as_str().unwrap())
	}
}

extern "system" {
	fn realpath(file_name:*c_char, resolved_name:*mut c_char) -> *c_char;
}

pub fn real_path(unresolved:&Path) -> Path {
	unresolved.with_c_str(|unres_buf| {
			let mut res_buf = [0 as c_char, ..os_specific::PATH_MAX + 1];
			unsafe { realpath(unres_buf, res_buf.as_mut_ptr()); }
			Path::new(c_buf_as_str(res_buf.as_slice()))
		})
}

fn is_mounted(mountpoint:&Path) -> bool {
	// Check whether the device number of our directory is the same as its
	// parent--this be the case only if the directory is not mounted.  If
	// we get an error trying to stat the mountpoint, it's probably because
	// it is mounted and the user filesystem is doing something weird--in
	// any case, we will make that assumption here.  If there's an error in
	// the stat of the parent, however, there is no explanation this code
	// can foresee and it will therefore just fail.
	io_error::cond.trap(|_:IoError| {}).inside(|| stat(mountpoint)).unstable.device != stat(&mountpoint.dir_path()).unstable.device
}

#[cfg(target_os = "macos")]
mod os_specific {
	pub static PATH_MAX:uint = 1024;

	pub fn unmount_program_and_args(mountpoint:&Path) -> (&'static str, ~[~str]) {
		("umount", box [mountpoint.display().to_str()])
	}
}

#[cfg(target_os = "linux")]
mod os_specific {
	pub static PATH_MAX:uint=4096;

	pub fn unmount_program_and_args(mountpoint:&Path) -> (&'static str, ~[~str]) {
		("fusermount", box [~"-u", mountpoint.display().to_str()])
	}
}

// Wait for a given function to return true, delaying a given number of msecs between each try,
// up to a given number of tries.  Returns true if the function returned true before the limit
// ran out.  Takes a name for logging/debug purposes
fn wait_for(tries: uint, msecs: u64, name: &str, f:||->bool) -> bool {
	!range_inclusive(0,tries).filter(|n| {
			if (*n != 0) {
				debug!("{:s} not detected; waiting {:u} msecs to try again (try {:u})", name, msecs, *n);
				sleep(msecs);
			}
			let res = f();
			if res {
				debug!("{:s} found after {:u} delayed retries", name, *n);
			}
			res
		}).next().is_none()
}

// Wait for a child process to finish, killing it if this does not happen within a given amount of time.
fn process_wait_with_timeout(process:&mut Process, name:&str, msecs:u64) -> ProcessExit {
	let (finish_port, finish_chan) = Chan::new();
	let timer = Timer::new().unwrap();  // Do this outside the child task so that we fail if we can't create the timer
	let pid = process.id();  // Can't send a reference to the Process object into the task, so we'll have to use libc's kill directly
	let name = name.into_owned();
	do spawn {
		let mut timer = timer;
		let mut finish_port = finish_port;
		let mut timeout_port = timer.oneshot(msecs);
		select!(
			_timed_out = timeout_port.recv() => {
				error!("Timed out after {} msecs waiting for {} to finish", msecs, name);
				unsafe { kill(pid, MustDieSignal as i32) };
			},
			_done = finish_port.recv() => {}
			);
	};
	let res = process.wait();
	// Race condition alert: The timeout could happen between right here, after wait() has returned
	// but before we send on the finish chan.  This would cause us to issue a libc kill on an
	// expired pid--which in theory could be a whole different process unrelated to this one!  In
	// practice this is so unlikely that we will consider it not worth worrying about in test-only
	// code like this.
	finish_chan.try_send(());
	res
}

struct UserFilesystemMount {
	temp_dir: TempDir,
	mountpoint: Path,
	fs_user_process: Process,
	name: ~str,
}
impl UserFilesystemMount {
	fn new(fs_binary: Path) -> UserFilesystemMount {
		let temp_dir = TempDir::new(fs_binary.filename_str().unwrap()).unwrap();
		let mountpoint = real_path(temp_dir.path());
		// FIXME: The use of display() here is not correct, but needed until somebody fixes mozilla/rust#9639
		let fs_user_process = Process::new(ProcessConfig{
				program: fs_binary.display().to_str(),
				args: [mountpoint.display().to_str()],
				env: None,
				cwd: None,
				io: [Ignored, InheritFd(1), InheritFd(2)]
			});
		debug!("Successfully started user FS binary: {}", fs_binary.display());
		let res = UserFilesystemMount {
			temp_dir: temp_dir,
			mountpoint: mountpoint,
			fs_user_process: fs_user_process.unwrap(),
			name: from_utf8(fs_binary.filename().unwrap()).into_owned()
		};
		res
	}

	fn wait_until_mounted(&self) {
		assert!(wait_for(20, 500, format!("Mount of {}", self.name), 
						 || is_mounted(&self.mountpoint)),
				"{} failed to mount on {}", self.name, self.mountpoint.display());
	}

	fn unmount(&self) {
		// We don't really care if this succeeds or fails.  If it failed, it's probably because
		// the FS process was unmounted already.
		let _g = io_error::cond.trap(|_:IoError| {}).guard();
		debug!("Unmounting {}", self.mountpoint.display());
		let (program, args) = os_specific::unmount_program_and_args(&self.mountpoint);
		Process::new(ProcessConfig{
				program: program,
				args: args,
				env: None,
				cwd: None,
				io: [Ignored, InheritFd(1), InheritFd(2)]
			});
	}

	/// Returns true if the process was successfully killed (i.e. was still running), false if not.
	fn kill(&mut self) -> bool {
		let mut res = true;
		io_error::cond.trap(|_:IoError| { res = false; }).inside(|| {
				self.fs_user_process.signal(MustDieSignal)
			});
		res
	}

	fn assert_withinprocess_test_succeeded(&mut self, timeout_msecs:u64) {
		let _g = io_error::cond.trap(|_:IoError| {}).guard();
		let pexit = process_wait_with_timeout(&mut self.fs_user_process, self.name, timeout_msecs);
		assert!(pexit.success(), "Within-process test {} failed", self.name);
	}
}
impl Drop for UserFilesystemMount {
	fn drop(&mut self) {
		debug!("Destroying mount for {}", self.name);
		self.unmount();
		let was_unmounted = wait_for(20, 500, format!("Unmount of {}", self.mountpoint.display()), || !is_mounted(&self.mountpoint));
		let still_running = !wait_for(20, 500, format!("Death of chid process for {}", self.name), || !self.kill());
		// If we did not successfully unmount the first time, try again after killing the child process
		if (!was_unmounted) {
			self.unmount();
		}
		// If we haven't failed already, do one last check to make sure the child process isn't
		// still running after we've unmounted the filesystem.
		if !std::task::failing() {
			assert!(was_unmounted, format!("Initial unmount was unsuccessful"));
			assert!(!still_running, format!("Child process was still running after file system unmount: {}", self.name));
		}
	}
}

fn path_from_self(name:&str) -> Path {
	Path::new(args()[0]).dir_path().join(name)
}

fn helper_path(name:&str) -> Path {
	path_from_self("integration-test-helpers").join(name)
}

#[test]
fn null_example() {
	let ufm = UserFilesystemMount::new(path_from_self("null"));
	ufm.wait_until_mounted();
	let mut error = None;
	io_error::cond.trap(|e:IoError| { error = Some(e); }).inside(|| {
			let nonexistant_file = File::open(&ufm.mountpoint.join("thisdoesnotexist"));
			assert!(nonexistant_file.is_none());
		});
	
	// This ensures that the error we got when opening the file really came from our null
	// filesystem, and not an ordinary "file not found" error
	assert_eq!(error.unwrap().kind, OtherIoError);
}

#[test]
fn hello_example() {
	let ufm = UserFilesystemMount::new(path_from_self("hello"));
	ufm.wait_until_mounted();
	let hello_contents = File::open(&ufm.mountpoint.join("hello.txt")).read_to_end();
	assert_eq!(hello_contents, "Hello World!\n".as_bytes().into_owned());
	io_error::cond.trap(|_:IoError| { }).inside(|| {
			let nonexistant_file = File::open(&ufm.mountpoint.join("thisdoesnotexist"));
			assert!(nonexistant_file.is_none());
		});
}

#[test]
fn fs_mounted_after_spawn_returns() {
	UserFilesystemMount::new(helper_path("fs-mounted-after-spawn-returns")).assert_withinprocess_test_succeeded(30000);
}
