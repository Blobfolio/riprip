/*!
# Rip Rip Hooray: Log
*/

use cdtoc::Track;
use std::io::Write;



/// # Super Basic Log.
///
/// This holds the log-worthy details from an individual pass, printing it out
/// en masse at the end of the run.
///
/// Doing it this way, versus printing each line in realtime, ensures
/// consistent ordering, otherwise it's a crapshoot.
pub(super) struct RipLog(Vec<u8>);

impl Drop for RipLog {
	/// # Final Print Maybe.
	fn drop(&mut self) { self.flush(); }
}

impl RipLog {
	/// # New Instance.
	pub(super) const fn new() -> Self { Self(Vec::new()) }

	/// # New Pass!
	pub(super) fn pass(&mut self, pass: u8) {
		self.flush();
		let _res = writeln!(
			&mut self.0,
			r"##
## Pass {pass}.
##");
	}

	/// # Log Something.
	pub(super) fn line<S>(&mut self, track: Track, lsn: i32, description: S)
	where S: AsRef<str> {
		let _res = writeln!(
			&mut self.0,
			"{:10}  {:02}  {lsn:06}  {}",
			utc2k::unixtime(),
			track.number(),
			description.as_ref().trim(),
		);
	}

	/// # Flush.
	fn flush(&mut self) {
		if ! self.0.is_empty() {
			let writer = std::io::stdout();
			let mut handle = writer.lock();
			let _res = handle.write_all(&self.0).and_then(|_| handle.flush());
			self.0.truncate(0);
		}
	}
}
