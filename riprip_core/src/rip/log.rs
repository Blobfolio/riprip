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
/// This holds the log-worthy details from an individual pass, printing the
/// records out — to STDOUT — en masse at the end of the run.
///
/// Aside from helping to ensure consistent formatting, this also keeps the
/// ordering consistent.
pub(super) struct RipLog {
	/// # Pass Number, Timestamp.
	pass: Option<(NonZeroU8, Instant)>,

	/// # Events.
	events: Vec<(RipLogEventKind, FmtUtc2k)>,

	/// # Sectors.
	///
	/// This holds each track number, LSN, sample count, and status.
	sectors: Vec<(u8, i32, u16, RipLogSampleKind)>,
}

impl Drop for RipLog {
	/// # Final Print (Maybe).
	///
	/// This will print any remaining log data before retiring.
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
	///
	/// This prints the contents of the previous pass, if any, and increments
	/// the pass counter so it can start all over again.
	pub(super) fn bump_pass(&mut self) {
		self.flush();

		// Unnecessary but unhurtful.
		self.events.truncate(0);
		self.sectors.truncate(0);

		let next = self.pass.map_or(NonZeroU8::MIN, |(p, _)| p.saturating_add(1));
		self.pass.replace((next, Instant::now()));
	}

	/// # Add Cache Bust.
	///
	/// Record that a cache bust occurred at such-and-such time.
	pub(super) fn add_cache_bust(&mut self) {
		self.events.push((RipLogEventKind::CacheBust, FmtUtc2k::now()));
	}

	/// # Add Error.
	///
	/// Record a read or sync error corresponding to a read attempt at `lsn`.
	pub(super) fn add_error(&mut self, lsn: i32, err: RipRipError) {
		self.events.push((RipLogEventKind::Err((lsn, err)), FmtUtc2k::now()));
	}

	/// # Add Bad Sample Count.
	///
	/// Record the number of bad samples (`total`) associated with `lsn`.
	pub(super) fn add_bad(&mut self, track: Track, lsn: i32, total: u16) {
		self.sectors.push((
			track.number(),
			lsn,
			u16::min(total, 588),
			RipLogSampleKind::Bad,
		));
	}

	/// # Add Confused Sample Count.
	///
	/// Record the number of confused samples (`total`) associated with `lsn`.
	/// These are samples the drive can't seem to make its mind up about; at
	/// least four different "good" values have to have been returned to
	/// qualify.
	pub(super) fn add_confused(&mut self, track: Track, lsn: i32, total: u16) {
		self.sectors.push((
			track.number(),
			lsn,
			u16::min(total, 588),
			RipLogSampleKind::Confused,
		));
	}

	/// # Flush.
	///
	/// Print the held data, if any, to STDOUT, and drain it so a new pass can
	/// start fresh.
	///
	/// This uses a locked writer so content should appear in the correct
	/// order, but one never knows with terminals…
	fn flush(&mut self) {
		// Header.
		let Some((pass, start)) = self.pass else { return; };
		let writer = std::io::stdout();
		let mut handle = writer.lock();
		let _res = writeln!(
			&mut handle,
			"##
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
				let _res = writeln!(&mut handle, "## [{time}] {event}");
			}
			let _res =writeln!(&mut handle, "##");
		}

		// Sample issues.
		if ! self.sectors.is_empty() {
			self.sectors.sort_unstable_by(|a, b| a.1.cmp(&b.1));
			for (track, lsn, samples, kind) in self.sectors.drain(..) {
				let _res = writeln!(
					&mut handle,
					"{track:02}  {lsn:06}  {samples:03}  {}",
					kind.as_str(),
				);
			}
		}

		// Write it!
		let _res = handle.flush();
	}
}



/// # Event Kind.
///
/// This is used to enable grouping of different kinds of "events", which at
/// present are just "we busted the cache" and read-related errors. There might
/// be more things to talk about some day.
enum RipLogEventKind {
	/// # Cache Bust.
	CacheBust,

	/// # Error.
	Err((i32, RipRipError)),
}

impl fmt::Display for RipLogEventKind {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::CacheBust => f.write_str("------ Cache mitigation."),
			Self::Err((lsn, e)) => write!(f, "{lsn:06} {e}"),
		}
	}
}



#[derive(Debug, Clone, Copy)]
/// # Sample Issue Kind.
///
/// As we're only logging problem sectors, the two main things worth mentioning
/// are C2/read errors and major inconsistencies.
enum RipLogSampleKind {
	/// # Explicit Error.
	Bad,

	/// # Inconsistent Reads.
	Confused,
}

impl RipLogSampleKind {
	/// # As Str.
	///
	/// This could be `Display`, but as they're just single words, `const`
	/// seemed the better route.
	const fn as_str(self) -> &'static str {
		match self {
			Self::Bad => "BAD",
			Self::Confused => "CONFUSED",
		}
	}
}
