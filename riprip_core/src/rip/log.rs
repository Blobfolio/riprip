/*!
# Rip Rip Hooray: Log
*/

use cdtoc::Track;
use crate::RipRipError;
use dactyl::NiceElapsed;
use std::{
	fmt,
	io::Write,
	num::NonZeroU8,
	time::Instant,
};
use utc2k::FmtUtc2k;



/// # Super Basic Log.
///
/// This holds the log-worthy details from an individual pass, printing it out
/// en masse at the end of the run.
///
/// Doing it this way, versus printing each line in realtime, ensures
/// consistent ordering, otherwise it's a crapshoot.
pub(super) struct RipLog {
	pass: Option<(NonZeroU8, Instant)>,
	events: Vec<(RipLogEventKind, FmtUtc2k)>,
	sectors: Vec<(u8, i32, u8, RipLogSampleKind)>,
}

impl Drop for RipLog {
	/// # Final Print Maybe.
	fn drop(&mut self) { self.flush(); }
}

impl RipLog {
	/// # New Instance.
	pub(super) const fn new() -> Self {
		Self {
			pass: None,
			events: Vec::new(),
			sectors: Vec::new(),
		}
	}

	/// # New Pass!
	pub(super) fn pass(&mut self, pass: u8) {
		self.flush();

		// Unnecessary but unhurtful.
		self.events.truncate(0);
		self.sectors.truncate(0);

		// This should never fail.
		if let Some(pass) = NonZeroU8::new(pass) {
			self.pass.replace((pass, Instant::now()));
		}
	}

	/// # Add Cache Bust.
	pub(super) fn add_cache_bust(&mut self) {
		self.events.push((RipLogEventKind::CacheBust, FmtUtc2k::now()));
	}

	/// # Add Error.
	pub(super) fn add_error(&mut self, lsn: i32, err: RipRipError) {
		self.events.push((RipLogEventKind::Err((lsn, err)), FmtUtc2k::now()));
	}

	/// # Add Bad Sample Count.
	pub(super) fn add_bad(&mut self, track: Track, lsn: i32, total: u8) {
		self.sectors.push((
			track.number(),
			lsn,
			total,
			RipLogSampleKind::Bad,
		));
	}

	/// # Add Confused Sample Count.
	pub(super) fn add_confused(&mut self, track: Track, lsn: i32, total: u8) {
		self.sectors.push((
			track.number(),
			lsn,
			total,
			RipLogSampleKind::Confused,
		));
	}

	/// # Flush.
	fn flush(&mut self) {
		// Header.
		let Some((pass, start)) = self.pass.take() else { return; };
		let writer = std::io::stdout();
		let mut handle = writer.lock();
		let _res = writeln!(
			&mut handle,
			r"##
## Pass {pass}: {}
## Problematic Sectors: {}
## Problematic Samples: {}
##",
			NiceElapsed::from(start),
			self.sectors.len(),
			self.sectors.iter().fold(0_usize, |acc, (_, _, v, _)| acc + usize::from(*v))
		);

		// Miscellaneous events.
		if ! self.events.is_empty() {
			for (event, time) in self.events.drain(..) {
				let _res = writeln!(&mut handle, r"## [{time}] {event}");
			}
			let _res =writeln!(&mut handle, "##");
		}

		// Sample issues.
		if ! self.sectors.is_empty() {
			self.sectors.sort_unstable_by(|a, b| a.1.cmp(&b.1));
			for (track, lsn, samples, kind) in self.sectors.drain(..) {
				let _res = writeln!(
					&mut handle,
					r"{track:02}  {lsn:06}  {samples:03}  {}",
					kind.as_str(),
				);
			}
		}

		// Write it!
		let _res = handle.flush();
	}
}



/// # Event Kind.
enum RipLogEventKind {
	CacheBust,
	Err((i32, RipRipError)),
}

impl fmt::Display for RipLogEventKind {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::CacheBust => f.write_str("------ Defeat drive cache."),
			Self::Err((lsn, e)) => write!(f, "{lsn:06} {e}"),
		}
	}
}



#[derive(Debug, Clone, Copy)]
/// # Sample Issue Kind.
enum RipLogSampleKind {
	Bad,
	Confused,
}

impl RipLogSampleKind {
	/// # As Str.
	const fn as_str(self) -> &'static str {
		match self {
			Self::Bad => "BAD",
			Self::Confused => "CONFUSED",
		}
	}
}
