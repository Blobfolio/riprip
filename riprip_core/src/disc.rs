/*!
# Rip Rip Hooray: Disc
*/

use cdtoc::{
	Toc,
	TocKind,
};
use crate::{
	Barcode,
	CacheWriter,
	CD_LEADIN,
	CD_LEADOUT_LABEL,
	CDTextKind,
	COLOR_BAD,
	COLOR_CONFIRMED,
	COLOR_LIKELY,
	DriveVendorModel,
	KillSwitch,
	LibcdioInstance,
	RipOptions,
	Ripper,
	RipRipError,
	SavedRips,
};
use fyi_msg::Progless;
use std::{
	borrow::Cow,
	collections::BTreeMap,
	ffi::OsStr,
	fmt,
	path::{
		Path,
		PathBuf,
	},
};



#[derive(Debug)]
/// # Disc.
///
/// A loaded and parsed compact disc.
pub struct Disc {
	cdio: LibcdioInstance,
	toc: Toc,
	barcode: Option<Barcode>,
	isrcs: BTreeMap<u8, String>,
}

impl fmt::Display for Disc {
	/// # Summarize the Disc.
	///
	/// This prints various disc identifiers and table of contents-type
	/// information in a nice little table.
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
			kv.push(("Barcode:", 199, barcode.to_string()));
		}

		let col_max: usize = kv.iter().map(|(k, _, _)| k.len()).max().unwrap_or(0);
		for (k, color, v) in kv {
			writeln!(f, "\x1b[1;38;5;{color}m{k:col_max$}\x1b[0m {v}")?;
		}

		// Start the table of contents.
		write!(
			f,
			"\n\x1b[2m##   FIRST    LAST  LENGTH          {}\x1b[0m\n",
			if self.isrcs.is_empty() { "" } else { "ISRC" },
		)?;
		f.write_str(DIVIDER)?;

		let mut total = 0;

		// HTOA.
		if let Some(t) = self.toc.htoa() {
			let rng = t.sector_range_normalized();
			let len = rng.end - rng.start;
			writeln!(
				f,
				"\x1b[2m00  {:>6}  {:>6}  {len:>6}          HTOA\x1b[0m",
				rng.start,
				rng.end - 1,
			)?;
		}
		// Leading data track.
		else if matches!(self.toc.kind(), TocKind::DataFirst) {
			total += 1;
			writeln!(
				f,
				"\x1b[2m{total:02}  {:>6}                    DATA TRACK\x1b[0m",
				self.toc.data_sector().unwrap_or_default().saturating_sub(u32::from(CD_LEADIN))
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
				"\x1b[2m{total:02}  {:>6}                    DATA TRACK\x1b[0m",
				self.toc.data_sector().unwrap_or_default().saturating_sub(u32::from(CD_LEADIN))
			)?;
		}

		// The leadout.
		writeln!(
			f,
			"\x1b[2m{CD_LEADOUT_LABEL}  {:>6}                      LEAD-OUT",
			self.toc.leadout().saturating_sub(u32::from(CD_LEADIN)),
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

		// Grab the leadout, then build the ToC.
		let leadout = cdio.leadout_lba()?;
		let toc = Toc::from_parts(audio, data, leadout)?;

		// Pull the barcode (if any).
		let barcode = cdio.mcn();

		// Pull the track ISRCs (if any).
		let mut isrcs = BTreeMap::default();
		for t in toc.audio_tracks() {
			let idx = t.number();
			if let Some(isrc) = cdio.cdtext(idx, CDTextKind::Isrc) {
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
	pub const fn barcode(&self) -> Option<Barcode> { self.barcode }

	#[must_use]
	/// # ISRC.
	pub fn isrc(&self, idx: u8) -> Option<&str> {
		self.isrcs.get(&idx).map(String::as_str)
	}

	#[must_use]
	#[inline]
	/// # Drive Vendor and Model.
	pub fn drive_vendor_model(&self) -> Option<DriveVendorModel> {
		self.cdio.drive_vendor_model()
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
	pub fn rip(&self, opts: &RipOptions, progress: &Progless, killed: &KillSwitch)
	-> Result<(), RipRipError> {
		// Handle all the ripping business!
		let mut rip = Ripper::new(self, opts)?;
		rip.rip(progress, killed)?;
		rip.summarize();

		// Mention all the file paths and statuses, and maybe build a cue
		// sheet to go along with them.
		if let Some(saved) = rip.finish() {
			let mut total = 0;
			let mut good = 0;

			let htoa_any = saved.contains_key(&0);
			let htoa_likely = saved.get(&0).map_or(false, |(_, ar, ctdb)| ar.is_some() || ctdb.is_some());
			let conf = saved.values().any(|(_, ar, ctdb)| ar.is_some() || ctdb.is_some());
			let col1 = saved.first_key_value().map_or(0, |(_, (dst, _, _))| dst.to_string_lossy().len());

			eprintln!("\nThe fruits of your labor:");

			// If we did all tracks, make a cue sheet.
			if let Some(file) = save_cuesheet(&self.toc, &saved) {
				eprintln!(
					"  \x1b[2m{}\x1b[0m",
					file.to_string_lossy(),
				);
			}

			for (idx, (file, ar, ctdb)) in saved {
				total += 1;
				if ar.is_some() || ctdb.is_some() { good += 1; }

				eprintln!(
					"  \x1b[2m{:<col1$}\x1b[0m{}{}",
					file.to_string_lossy(),
					if conf {
						if idx == 0 { Cow::Borrowed("            \x1b[0;93m*\x1b[0m") }
						else { fmt_ar(ar) }
					} else { Cow::Borrowed("            \x1b[0;91mx\x1b[0m") },
					if conf {
						if idx == 0 { Cow::Borrowed("         \x1b[0;93m*\x1b[0m") }
						else { fmt_ctdb(ctdb) }
					} else { Cow::Borrowed("         \x1b[0;91mx\x1b[0m") },
				);
			}

			// Add confirmation column headers.
			eprintln!(
				"  {}  AccurateRip  CUETools  \x1b[2m(\x1b[0;{}m{good}\x1b[0;2m/\x1b[0m{total}\x1b[2m)\x1b[0m",
				" ".repeat(col1),
				if good == 0 { COLOR_BAD } else { COLOR_CONFIRMED },
			);

			// Mention that the HTOA can't be verified but is probably okay.
			if htoa_likely {
				eprintln!("\n\x1b[{COLOR_LIKELY}m*\x1b[0;2m HTOA tracks cannot be verified w/ AccurateRip or CTDB,");
				eprintln!("  but this rip rates \x1b[0;{COLOR_LIKELY}mlikely\x1b[0;2m, which is the next best thing!\x1b[0m");
			}
			// Mention that the HTOA can't be verified and should be reripped
			// to increase certainty.
			else if htoa_any {
				eprintln!("\n\x1b[{COLOR_LIKELY}m*\x1b[0;2m HTOA tracks cannot be verified w/ AccurateRip or CTDB");
				eprintln!("  so you should re-rip it until it rates \x1b[0;{COLOR_LIKELY}mlikely\x1b[0;2m to be safe.\x1b[0m");
			}

			// An extra line break for separation.
			eprintln!();
		}

		Ok(())
	}
}



/// # Format AccurateRip.
fn fmt_ar(ar: Option<(u8, u8)>) -> Cow<'static, str> {
	if let Some((v1, v2)) = ar {
		let c1 =
			if v1 == 0 { COLOR_BAD }
			else if v1 <= 5 { COLOR_LIKELY }
			else { COLOR_CONFIRMED };

		let c2 =
			if v2 == 0 { COLOR_BAD }
			else if v2 <= 5 { COLOR_LIKELY }
			else { COLOR_CONFIRMED };

		Cow::Owned(format!(
			"        \x1b[0;{c1}m{:02}\x1b[0;2m+\x1b[0;{c2}m{:02}\x1b[0m",
			v1.min(99),
			v2.min(99),
		))
	}
	else { Cow::Borrowed("             ") }
}

#[allow(clippy::option_if_let_else)]
/// # Format CUETools.
fn fmt_ctdb(ctdb: Option<u16>) -> Cow<'static, str> {
	if let Some(v1) = ctdb {
		let c1 =
			if v1 == 0 { COLOR_BAD }
			else if v1 <= 5 { COLOR_LIKELY }
			else { COLOR_CONFIRMED };

		Cow::Owned(format!(
			"       \x1b[0;{c1}m{:03}\x1b[0m",
			v1.min(999),
		))
	}
	else { Cow::Borrowed("          ") }
}

/// # Generate CUE Sheet if Complete.
fn save_cuesheet(toc: &Toc, ripped: &SavedRips) -> Option<PathBuf> {
	use std::fmt::Write;

	// Make sure all tracks on the disc have been ripped, and pair their file
	// names with the corresponding Track object.
	let mut all = Vec::with_capacity(ripped.len());
	for track in toc.audio_tracks() {
		let (dst, _, _) = ripped.get(&track.number())?;
		let dst = dst.file_name().and_then(OsStr::to_str)?;
		all.push((track, dst));
	}

	// The output folder.
	let parent = ripped.get(&1).and_then(|(dst, _, _)| dst.parent())?;

	let mut cue = String::new();
	for (track, src) in all {
		// If there's an HTOA, it needs to be grouped with the first track.
		if track.position().is_first() && toc.htoa().is_some() {
			// This should have been ripped with everything else.
			let src0 = ripped.get(&0)
				.and_then(|(dst, _, _)| dst.file_name())
				.and_then(OsStr::to_str)?;

			// Add the lines to our cue!
			writeln!(&mut cue, "FILE \"{src0}\" WAVE").ok()?;
			cue.push_str("  TRACK 01 AUDIO\n");
			cue.push_str("    INDEX 00 00:00:00\n");
			writeln!(&mut cue, "FILE \"{src}\" WAVE").ok()?;
			cue.push_str("    INDEX 01 00:00:00\n");

			// We're done with tracks zero/one.
			continue;
		}

		// All other tracks are just file/track/index.
		writeln!(&mut cue, "FILE \"{src}\" WAVE").ok()?;
		writeln!(&mut cue, "  TRACK {:02} AUDIO", track.number()).ok()?;
		cue.push_str("    INDEX 01 00:00:00\n");
	}

	// Save the cue sheet!
	let dst = parent.join(format!("{}.cue", toc.cddb_id()));
	{
		use std::io::Write;
		let mut writer = CacheWriter::new(&dst).ok()?;
		writer.writer().write_all(cue.as_bytes()).ok()?;
		writer.finish().ok()?;
	}

	// Return the path.
	Some(dst)
}
