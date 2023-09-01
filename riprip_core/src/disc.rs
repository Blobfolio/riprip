/*!
# Rip Rip Hooray: Disc
*/

use cdtoc::{
	Toc,
	TocKind,
};
use crate::{
	cache_path,
	CD_LEADIN,
	CD_LEADOUT_LABEL,
	CDTextKind,
	KillSwitch,
	LibcdioInstance,
	Rip,
	RipOptions,
	RipRipError,
};
use fyi_msg::{
	Msg,
	Progless,
};
use std::{
	borrow::Cow,
	collections::BTreeMap,
	fmt,
	path::Path,
};



#[derive(Debug)]
/// # Disc.
///
/// A loaded and parsed compact disc.
pub struct Disc {
	cdio: LibcdioInstance,
	toc: Toc,
	barcode: Option<String>,
	isrcs: BTreeMap<u8, String>,
}

impl fmt::Display for Disc {
	#[inline]
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		const DIVIDER: &str = "\x1b[2m----------------------------------------\x1b[0m\n";

		// A few key/value pairs.
		let mut kv: Vec<(&str, u8, String)> = vec![
			("CDTOC:", 199, self.toc.to_string()),
			("AccurateRip:", 4, self.toc.accuraterip_id().to_string()),
			("CDDB:", 4, self.toc.cddb_id().to_string()),
			("CUETools:", 4, self.toc.ctdb_id().to_string()),
			("MusicBrainz:", 4, self.toc.musicbrainz_id().to_string()),
		];
		if let Some(barcode) = self.barcode.as_ref() {
			kv.push(("Barcode:", 199, barcode.clone()));
		}

		let col_max: usize = kv.iter().map(|(k, _, _)| k.len()).max().unwrap_or(0);
		for (k, color, v) in kv {
			writeln!(f, "\x1b[1;38;5;{color}m{k:col_max$}\x1b[0m {v}")?;
		}

		// Start the table of contents.
		f.write_str("\n\x1b[2m##   FIRST    LAST  LENGTH          ISRC\x1b[0m\n")?;
		f.write_str(DIVIDER)?;

		// Leading data track.
		let mut total = 0;
		if matches!(self.toc.kind(), TocKind::DataFirst) {
			total += 1;
			writeln!(
				f,
				"\x1b[2m{total:02}  {:>6}                  DATA TRACK\x1b[0m",
				self.toc.data_sector().unwrap_or_default().saturating_sub(CD_LEADIN)
			)?;
		}

		// The audio tracks.
		for t in self.toc.audio_tracks() {
			total += 1;
			let num = t.number();
			let rng = t.sector_range_normalized();
			let len = rng.end - rng.start;
			let isrc = self.isrc(num).unwrap_or_default();
			writeln!(
				f,
				"{num:02}  {:>6}  {:>6}  {len:>6}  {isrc:>12}",
				rng.start,
				rng.end - 1,
			)?;
		}

		// Trailing data track.
		if matches!(self.toc.kind(), TocKind::CDExtra) {
			total += 1;
			writeln!(
				f,
				"\x1b[2m{total:02}  {:>6}                  DATA TRACK\x1b[0m",
				self.toc.data_sector().unwrap_or_default().saturating_sub(CD_LEADIN)
			)?;
		}

		// The leadout.
		writeln!(
			f,
			"\x1b[2m{CD_LEADOUT_LABEL}  {:>6}                    LEAD-OUT",
			self.toc.leadout()
		)?;

		// Close it off!
		f.write_str(DIVIDER)?;
		writeln!(f)
	}
}

impl Disc {
	/// # New.
	///
	/// Load and parse the basic disc structure!
	///
	/// ## Errors
	///
	/// This will return an error if there's a problem communicating with the
	/// drive, the disc is unsupported, etc.
	pub fn new<P>(dev: Option<P>) -> Result<Self, RipRipError>
	where P: AsRef<Path> {
		let cdio = LibcdioInstance::new(dev)?;

		// Parse the table of contents into the pieces needed for `Toc`.
		let mut audio = Vec::new();
		let mut data = None;

		// The inclusive range to search.
		let from = cdio.first_track_num()?;
		let to = cdio.num_tracks()?;
		if to < from { return Err(RipRipError::NumTracks); }

		// Grab the position and type for each track.
		for idx in from..=to {
			let start = cdio.track_lba_start(idx)?;
			if cdio.track_format(idx)? {
				audio.push(start);
			}
			else {
				if data.is_some() || (idx != 1 && idx != to) {
					return Err(RipRipError::TrackFormat(idx));
				}
				data.replace(start);
			}
		}

		// We should have at least one audio track, but just in case…
		if audio.is_empty() { return Err(RipRipError::NoTracks); }

		// Grab the leadout, then build the ToC.
		let leadout = cdio.leadout_lba()?;
		let toc = Toc::from_parts(audio, data, leadout)?;

		// Pull the barcode (if any).
		let barcode = cdio.cdtext(0, CDTextKind::Barcode).or_else(|| cdio.mcn());

		// Pull the track ISRCs (if any).
		let mut isrcs = BTreeMap::default();
		for t in toc.audio_tracks() {
			let idx = t.number();
			if let Some(isrc) = cdio.cdtext(idx, CDTextKind::Isrc).or_else(|| cdio.track_isrc(idx)) {
				isrcs.insert(idx, isrc);
			}
		}

		// Finally done!
		Ok(Self { cdio, toc, barcode, isrcs })
	}
}

impl Disc {
	#[must_use]
	/// # Table of Contents.
	pub const fn toc(&self) -> &Toc { &self.toc }

	#[must_use]
	/// # Barcode.
	pub fn barcode(&self) -> Option<&str> { self.barcode.as_deref() }

	#[must_use]
	/// # ISRC.
	pub fn isrc(&self, idx: u8) -> Option<&str> {
		self.isrcs.get(&idx).map(String::as_str)
	}

	#[must_use]
	/// # Internal CDIO.
	pub(super) const fn cdio(&self) -> &LibcdioInstance { &self.cdio }
}

impl Disc {
	/// # Rip!
	///
	/// Rip the disc using the chosen options, extracting the track(s)
	/// afterward.
	///
	/// ## Errors
	///
	/// This will bubble up any IO/rip/etc. errors encountered along the way.
	pub fn rip(&self, opt: &RipOptions, progress: &Progless, killed: &KillSwitch)
	-> Result<(), RipRipError> {
		// Are we ripping specific tracks or everything?
		let mut tracks = Cow::Borrowed(opt.tracks());
		if tracks.is_empty() {
			tracks = Cow::Owned(self.toc.audio_tracks().map(|t| t.number()).collect());
		}

		// Loop the loop!
		let mut saved = Vec::new();
		for &t in &*tracks {
			if killed.killed() { continue; }

			let Some(track) = self.toc.audio_track(usize::from(t)) else {
				Msg::warning(format!("There is no audio track #{t}.")).eprint();
				continue;
			};

			// Continue from a previous run?
			let mut rip = Rip::new(self.toc.accuraterip_id(), track)?;
			if let Some(dst) = rip.rip(self, opt, progress, killed)? {
				saved.push(dst);
			}
		}

		// Print what we did!
		if ! saved.is_empty() {
			eprintln!("\nThe fruits of your labor:");
			for file in saved {
				if let Ok(file) = cache_path(file) {
					if file.is_file() {
						eprintln!("  \x1b[2m{}\x1b[0m", file.to_string_lossy());
					}
				}
			}
			eprintln!();
		}

		Ok(())
	}
}
