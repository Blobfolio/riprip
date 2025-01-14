/*!
# Rip Rip Hooray: Kill Switch
*/

use std::sync::{
	Arc,
	atomic::{
		AtomicBool,
		Ordering::Acquire,
	},
};



#[derive(Debug, Clone, Copy)]
/// # Kill Switch.
///
/// This is a short-circuit for long-running operations across multiple
/// threads. (Ripping is single-threaded, but the progress bar isn't.)
///
/// The main program's CTRL-C intercept sets the value, allowing Rip Rip to
/// tidy up before dying.
pub struct KillSwitch(&'static Arc<AtomicBool>);

impl From<&'static Arc<AtomicBool>> for KillSwitch {
	#[inline]
	fn from(src: &'static Arc<AtomicBool>) -> Self { Self(src) }
}

impl KillSwitch {
	#[must_use]
	/// # Dead?
	pub fn killed(&self) -> bool { self.0.load(Acquire) }
}
