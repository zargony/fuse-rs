use std::util::NonCopyable;
/// A `Reply` signifies that either a reply was provided already to a file system operation, or a
/// task was started that will provide such a reply.  It can only be created by from a Request
/// object.
pub struct Reply {
	// This type does not contain anything.  It's just a token that lets the type system force the
	// user to provide a reply.

	priv nocopies: NonCopyable,

	// This appears to be necessary to work around a bug in the rust compiler (mozilla/rust#10028)
	// in which the compiler crashes on certain cases when returning a zero-sized struct from a function.
	priv make_me_have_nonzero_size:u8
}

// This is not inside an impl of Reply because then it would get imported with Reply itself.  But
// the only reason we even have the Reply token is that an API user should not be able to create
// one, so we don't want that.
pub fn reply_token() -> Reply { Reply{nocopies: NonCopyable, make_me_have_nonzero_size:0} }
