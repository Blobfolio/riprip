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



#[derive(Debug)]
/// # Kill Switch.
///
/// This is a short-circuit for long-running operations across multiple
/// threads. (Ripping is single-threaded, but the progress bar isn't.)
///
/// The main program's CTRL-C intercept sets the value, allowing Rip Rip to
/// tidy up before dying.
pub struct KillSwitch(Arc<AtomicBool>);

impl Default for KillSwitch {
	fn default() -> Self { Self(Arc::from(AtomicBool::new(false))) }
}

impl KillSwitch {
	#[must_use]
	/// # Dead?
	pub fn killed(&self) -> bool { self.0.load(Acquire) }

	#[must_use]
	/// # Inner Clone.
	pub fn inner(&self) -> Arc<AtomicBool> { Arc::clone(&self.0) }
}
