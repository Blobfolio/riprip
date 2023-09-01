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
/// This is used to potentially short-circuit long-running arguments across
/// threads.
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
