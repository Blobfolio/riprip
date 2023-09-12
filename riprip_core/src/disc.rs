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
	DriveVendorModel,
	KillSwitch,
	LibcdioInstance,
	Rip,
	RipOptions,
	RipRipError,
	SAMPLES_PER_SECTOR,
	WAVE_SPEC,
};
use fyi_msg::{
	Msg,
	Progless,
};
use hound::WavWriter;
use std::{
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

		// Leading data track.
		let mut total = 0;
		if matches!(self.toc.kind(), TocKind::DataFirst) {
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
		// Loop the loop!
		let mut saved = BTreeMap::default();
		for t in opts.tracks() {
			if killed.killed() { continue; }

			let Some(track) = self.toc.audio_track(usize::from(t)) else {
				Msg::warning(format!("There is no audio track #{t}.")).eprint();
				continue;
			};

			// Rip it, and keep track of the destination file so we can print
			// a complete list at the end.
			let mut rip = Rip::new(self, track, opts)?;
			let res = rip.rip(progress, killed)?;
			rip.summarize();
			saved.insert(t, res);
		}

		// Print what we did!
		if ! saved.is_empty() {
			eprintln!("\nThe fruits of your labor:");

			// If we did all tracks, make a cue sheet.
			if ! opts.raw() {
				if let Some(file) = save_cuesheet(&self.toc, &saved) {
					eprintln!("  \x1b[2m{}\x1b[0m", file.to_string_lossy());
				}
			}

			for (file, confirmed) in saved.values() {
				eprintln!(
					"  \x1b[2m{}{}\x1b[0m",
					file.to_string_lossy(),
					if *confirmed { " \x1b[0;1;92m✓" } else { "" },
				);
			}
			eprintln!();
		}

		Ok(())
	}
}



/// # Generate CUE Sheet if Complete.
fn save_cuesheet(toc: &Toc, ripped: &BTreeMap<u8, (PathBuf, bool)>) -> Option<PathBuf> {
	use std::fmt::Write;

	// Make sure all tracks on the disc have been ripped, and pair their file
	// names with the corresponding Track object.
	let mut all = Vec::with_capacity(ripped.len());
	for track in toc.audio_tracks() {
		let (dst, _) = ripped.get(&track.number())?;
		let dst = dst.file_name().and_then(OsStr::to_str)?;
		all.push((track, dst));
	}

	// The output folder.
	let parent = ripped.get(&1).and_then(|(dst, _)| dst.parent())?;

	let mut cue = String::new();
	for (track, src) in all {
		// If the first track has a non-zero start, we need to generate the
		// pregap and write both their entries a little differently than we
		// otherwise would.
		if track.position().is_first() {
			let rng = track.sector_range_normalized();
			if rng.start != 0 {
				// Generate an output path for our 00 track.
				let dst = parent.join("00.wav");

				// CD samples are stereo pairs, but hound treats each channel
				// separately, so the number of hound-samples we'll write are
				// double.
				let len = rng.start.checked_mul(u32::from(SAMPLES_PER_SECTOR) * 2)?;

				// Write the wav.
				let mut writer = CacheWriter::new(&dst).ok()?;
				{
					let mut wav = WavWriter::new(writer.writer(), WAVE_SPEC).ok()?;
					let mut wav_writer = wav.get_i16_writer(len);
					for _ in 0..len { wav_writer.write_sample(0_i16); }
					wav_writer.flush().ok()?;
					wav.flush().ok()?;
					wav.finalize().ok()?;
				}
				writer.finish().ok()?;

				// Add the lines to our cue!
				cue.push_str("FILE \"00.wav\" WAVE\n");
				cue.push_str("  TRACK 01 AUDIO\n");
				cue.push_str("    INDEX 00 00:00:00\n");
				writeln!(&mut cue, "FILE \"{src}\" WAVE").ok()?;
				cue.push_str("    INDEX 01 00:00:00\n");

				// We're done with tracks zero/one.
				continue;
			}
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
