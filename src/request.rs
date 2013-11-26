/*!
 * A request represents information about an operation the kernel driver
 * wants us to perform.
 */

use std::libc::{c_int, EIO};
use channel::FuseChan;
use reply::{reply_token, Reply};
use sendable::{Sendable};
use std::task;

/// Function to handle replies to requests
pub type ReplyHandler<T> = proc(&FuseChan, u64, Result<T, c_int>);

/// Each filesystem operation has its own Request object, which must be used to get a Reply.
pub struct Request<T> {
	priv chan: FuseChan,
	priv unique: u64,
	replied: bool,
	priv reply_handler: Option<ReplyHandler<T>>
}

impl<T> Request<T> {
	/// Reply to a filesystem operation synchronously.  The reply is sent immediately--when this
	/// function returns, it has already been read and sent to the kernel.
	pub fn reply(mut self, result: Result<T, c_int>) -> Reply {
		self.reply_handler.take_unwrap()(&self.chan, self.unique, result);
		self.replied = true;
		reply_token()
	}

	/// Reply to a filesystem operation asynchronously.  A new task is spawned, with the scheduler
	/// as specified by `sched_mode`, to run the passed function.  The passed function must reply to
	/// the operation, and is given the request object to allow it to do so.
	pub fn reply_async(self, sched_mode: task::SchedMode, func: proc(req: Request<T>) -> Reply) -> Reply {
		let mut builder = task::task();
		builder.sched_mode(sched_mode);
		builder.spawn(|| { func(self); });
		reply_token()
	}

	/// Return a unique key that identifies this request.  This key is used when calling `interrupt`
	/// to indicate which request is being interrupted.  Note that the uniqueness of this value is
	/// only over _active_ requests.  After a reply has been sent for a request, the same "unique"
	/// value may be used for an unrelated subsequent request in the same session.
	pub fn unique(&self) -> u64 { self.unique }
}

#[unsafe_destructor]
impl<T> Drop for Request<T> {
	fn drop(&mut self) {
		if !self.replied {
			// The request fell out of scope without calling `reply`--probably due to failure.  Pass
			// an error back to the kernel to avoid a hanging i/o operation.
			error!("FS request({:u}) dropped without reply.  Sending error", self.unique);
			send_reply::<()>(&self.chan, self.unique, Err(EIO));
		}
	}
}

/// Reply to a request with the given data or error code
pub fn send_reply<T: Sendable> (ch: &FuseChan, unique: u64, result: Result<T, c_int>) {
	match result {
		Ok(reply) => {
			ch.send(unique, 0, &reply);
		},
		Err(err) => {
			ch.send(unique, -err, &());
		},
	}
}

/// Create a new request
pub fn new_request<T:Sendable>(chan:FuseChan, unique:u64) -> Request<T> {
	new_request_with_handler(chan, unique, send_reply)
}

pub fn new_request_with_handler<T>(chan:FuseChan, unique: u64, handler: ReplyHandler<T>) -> Request<T> {
	Request { chan: chan, unique: unique, replied: false, reply_handler: Some(handler) }
}
