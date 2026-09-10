/*!
# Rip Rip Hooray: Disc
*/

use cdtoc::{
	Toc,
	TocKind,
};
use crate::{
	Barcode,
	cache_path,
	cache_prefix,
	CacheWriter,
	CD_LEADOUT_LABEL,
	CddaDriver,
	CddaDriverExt,
	cdtext::{
		CDText,
		CDTextError,
		DiscField,
		TrackField,
	},
	DriveVendorModel,
	KillSwitch,
	macros::log,
	RipOptions,
	Ripper,
	RipRipError,
	SavedRips,
};
use dactyl::NoHash;
use fyi_msg::{
	fyi_ansi::{
		ansi,
		csi,
		dim,
	},
	Msg,
	Progless,
};
use std::{
	borrow::Cow,
	collections::HashMap,
	ffi::OsStr,
	fmt,
	io::StderrLock,
	path::{
		Path,
		PathBuf,
	},
};



/// # Disc.
///
/// A loaded and parsed compact disc.
pub struct Disc {
	/// # CDIO Instance.
	cdda: CddaDriver,

	/// # Disc Table of Contents.
	toc: Toc,

	/// # CD-Text.
	cdtext: Option<(Vec<u8>, Result<CDText, CDTextError>)>,

	/// # Barcode.
	barcode: Option<Barcode>,

	/// # Track ISRCs.
	isrcs: HashMap<u8, String, NoHash>,
}

impl fmt::Display for Disc {
	/// # Summarize the Disc.
	///
	/// This prints various disc identifiers and table of contents-type
	/// information in a nice little table.
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		/// # Divider.
		const DIVIDER: &str = dim!("----------------------------------------\n");

		// A few key/value pairs.
		let mut kv: Vec<(&str, &str, String)> = vec![
			("CDTOC:", csi!(bold, 199), self.toc.to_string()),
			("AccurateRip:", csi!(bold, blue), self.toc.accuraterip_id().to_string()),
			("CDDB:", csi!(bold, blue), cache_prefix(&self.toc).to_owned()),
			("CUETools:", csi!(bold, blue), self.toc.ctdb_id().to_string()),
			("MusicBrainz:", csi!(bold, blue), self.toc.musicbrainz_id().to_string()),
		];
		if let Some(barcode) = self.barcode.as_ref() {
			kv.push(("Barcode:", csi!(bold, 199), barcode.to_string()));
		}

		let col_max: usize = kv.iter().map(|(k, _, _)| k.len()).max().unwrap_or(0);
		for (k, color, v) in kv {
			writeln!(
				f,
				concat!("{color}{k:col_max$}", csi!(), " {v}"),
				color=color,
				k=k,
				col_max=col_max,
				v=v,
			)?;
		}

		// Start the table of contents.
		write!(
			f,
			dim!("\nNO   FIRST    LAST  LENGTH          {}\n"),
			if self.has_isrcs() { "ISRC" } else { "" },
		)?;
		f.write_str(DIVIDER)?;

		let mut total = 0;

		// HTOA.
		if let Some(t) = self.toc.htoa() {
			let rng = t.sector_range_normalized();
			let len = rng.end - rng.start;
			writeln!(
				f,
				dim!("00  {:>6}  {:>6}  {:>6}          HTOA"),
				rng.start,
				rng.end - 1,
				len,
			)?;
		}
		// Leading data track.
		else if matches!(self.toc.kind(), TocKind::DataFirst) {
			total += 1;
			writeln!(
				f,
				dim!("{:02}  {:>6}                    DATA TRACK"),
				total,
				self.toc.data_sector_normalized().unwrap_or_default(),
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
				dim!("{:02}  {:>6}                    DATA TRACK"),
				total,
				self.toc.data_sector_normalized().unwrap_or_default(),
			)?;
		}

		// The leadout.
		writeln!(
			f,
			concat!(csi!(dim), "{}  {:>6}                      LEAD-OUT"),
			CD_LEADOUT_LABEL,
			self.toc.leadout_normalized(),
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
	pub fn new<P>(dev: Option<P>, cdtext: bool)
	-> Result<Self, RipRipError>
	where P: AsRef<Path> {
		let cdda = CddaDriver::new(dev)?;

		// Parse the table of contents into the pieces needed for `Toc`.
		let mut audio = Vec::new();
		let mut data = None;

		// The inclusive range to search.
		let from = cdda.first_track_num()?;
		let to = cdda.num_tracks()?;
		if to < from { return Err(RipRipError::NumTracks); }

		// Grab the position and type for each track.
		for idx in from..=to {
			let start = cdda.track_lba_start(idx)?;
			if cdda.track_format(idx)? {
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
		let leadout = cdda.leadout_lba()?;
		let toc = Toc::from_parts(audio, data, leadout)?;

		// We have most of it.
		let mut out = Self {
			cdda,
			toc,
			cdtext: None,
			barcode: None,
			isrcs: HashMap::with_hasher(NoHash::default()),
		};

		// Unless the user opted out of CD-Text parsing, let's handle that
		// now.
		if cdtext && let Some(raw_cdtext) = out.cdda.cdtext() {
			match CDText::from_bytes(&raw_cdtext) {
				Ok(cdtext) => {
					// Set the barcode.
					out.barcode = cdtext.disc(DiscField::Barcode)
						.find_map(|v| Barcode::try_from(v.as_bytes()).ok())
						.or_else(|| out.cdda.mcn_subchannel());

					// Pull the track ISRCs (if any).
					for t in out.toc.audio_tracks() {
						let idx = t.number();
						if let Some(isrc) = cdtext.track(TrackField::Isrc, idx).next() {
							out.isrcs.insert(idx, isrc.to_owned());
						}
					}

					out.cdtext.replace((raw_cdtext, Ok(cdtext)));
				},
				Err(e) => {
					out.cdtext.replace((raw_cdtext, Err(e)));
				},
			}
		}

		// Finally done!
		Ok(out)
	}
}

impl Disc {
	#[must_use]
	/// # Barcode.
	pub const fn barcode(&self) -> Option<Barcode> { self.barcode }

	#[must_use]
	/// # CD-Text.
	pub const fn cdtext(&self) -> Option<&CDText> {
		if let Some((_, Ok(v))) = self.cdtext.as_ref() { Some(v) }
		else { None }
	}

	#[must_use]
	#[inline]
	/// # Drive Vendor and Model.
	pub fn drive_vendor_model(&self) -> Option<DriveVendorModel> {
		self.cdda.drive_vendor_model()
	}

	#[must_use]
	/// # Has ISRC Data?
	pub fn has_isrcs(&self) -> bool { ! self.isrcs.is_empty() }

	#[must_use]
	/// # ISRC.
	pub fn isrc(&self, idx: u8) -> Option<&str> {
		self.isrcs.get(&idx).map(String::as_str)
	}

	#[must_use]
	/// # Table of Contents.
	pub const fn toc(&self) -> &Toc { &self.toc }

	#[must_use]
	/// # Internal CDIO.
	pub(super) const fn cdda(&self) -> &CddaDriver { &self.cdda }
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
	pub fn rip(&self, opts: &RipOptions, progress: &Progless, killed: KillSwitch)
	-> Result<(), RipRipError> {
		use std::io::Write;

		// Handle all the ripping business!
		let mut rip = Ripper::new(self, opts)?;
		rip.rip(progress, killed)?;
		rip.summarize();

		// Mention all the file paths and statuses, and maybe build a cue
		// sheet to go along with them.
		if let Some(saved) = rip.finish() {
			let mut handle = std::io::stderr().lock();
			let mut total = 0;
			let mut good = 0;

			let htoa_any = saved.contains_key(&0);
			let htoa_likely = saved.get(&0).is_some_and(|(_, ar, ctdb)| ar.is_some() || ctdb.is_some());
			let conf = saved.values().any(|(_, ar, ctdb)| ar.is_some() || ctdb.is_some());
			let col1 = saved.first_key_value().map_or(0, |(_, (dst, _, _))| dst.to_string_lossy().len());

			// A header of sorts.
			let _res = writeln!(&mut handle, "\nThe fruits of your labor:");
			log!(@info "Finished rip.{}", LoggableFruits(&saved));

			// If we did all tracks, make a cue sheet and print its path.
			if let Some(file) = save_cuesheet(&self.toc, &saved) {
				let _res = writeln!(&mut handle, dim!("  {}"), file.display());
				log!(@info "Saved cuesheet.\n  {}", file.display());
			}

			// Print the verification status for all track(s).
			for (idx, (file, ar, ctdb)) in saved {
				total += 1;
				if ar.is_some() || ctdb.is_some() { good += 1; }

				let _res = writeln!(
					&mut handle,
					concat!(dim!("  {:<col1$}"), "{}{}"),
					file.display(),
					if conf {
						if idx == 0 { Cow::Borrowed(ansi!((reset, light_yellow) "            *")) }
						else { fmt_ar(ar, true) }
					} else { Cow::Borrowed(ansi!((reset, light_red) "            x")) },
					if conf {
						if idx == 0 { Cow::Borrowed(ansi!((reset, light_yellow) "         *")) }
						else { fmt_ctdb(ctdb, true) }
					} else { Cow::Borrowed(ansi!((reset, light_red) "         x")) },
					col1=col1,
				);
			}

			// Add confirmation column headers.
			let _res = writeln!(
				&mut handle,
				concat!(
					"  {line: >width$}  AccurateRip  CUETools  ",
					csi!(dim), "(",
					"{color}{good}",
					ansi!((reset, dim) "/"),
					"{total}",
					dim!(")"),
				),
				line="",
				width=col1,
				color=if good == 0 { csi!(reset, light_red) } else { csi!(reset, light_green) },
				good=good,
				total=total
			);

			// Add HTOA footnote, if applicable.
			if htoa_likely { write_htoa_likely(&mut handle); }
			else if htoa_any { write_htoa_any(&mut handle); }

			// Add an extra line break for separation, flush, and quit.
			let _res = writeln!(&mut handle).and_then(|()| handle.flush());
		}

		Ok(())
	}

	/// # Save CD-Text Data.
	///
	/// Try to save the raw and decoded CD-Text data to disk.
	pub fn save_cdtext(&self, progress: &Progless) {
		let Some((bin, txt)) = &self.cdtext else { return; };
		let prefix = cache_prefix(&self.toc);

		// Save the raw binary data first, unless it already exists.
		if
			let Ok(dst_bin) = cache_path(format!("{prefix}.cdtext.bin")) &&
			(
				! dst_bin.is_file() ||
				std::fs::read(&dst_bin).ok().is_none_or(|v| v != *bin)
			) &&
			CacheWriter::oneshot(&dst_bin, bin).is_err()
		{
			// This shouldn't fail, but if it does, we won't be able to save
			// the other version either.
			std::hint::cold_path();
			return;
		}

		// Same for the decoded version.
		match txt {
			Ok(txt) => {
				let txt = txt.to_string();
				if
					let Ok(dst_txt) = cache_path(format!("{prefix}.cdtext.txt")) &&
					(
						! dst_txt.is_file() ||
						std::fs::read_to_string(&dst_txt).ok().is_none_or(|v| v != txt)
					) &&
					CacheWriter::oneshot(&dst_txt, txt.as_bytes()).is_err()
				{
					std::hint::cold_path();
				}
			},

			// If decoding had failed due to a feature-related issue, ask the
			// user to open a bug report.
			Err(CDTextError::UnsupportedDoubleByte | CDTextError::UnsupportedEncoding | CDTextError::UnsupportedExtension) => {
				let _res = progress.push_msg(Msg::warning(format!(
					concat!(
					"Rip Rip wasn't able to decode the CD-Text. Please consider sharing\n",
					"         the ", dim!("{prefix}.cdtext.bin"), " so we can fix that!\n",
					ansi!((light_blue) "         https://github.com/Blobfolio/riprip/issues/new"),
					),
					prefix=prefix,
				)));
			},

			// If decoding had failed for some other reason, do nothing.
			_ => {},
		}
	}

	/// # Status.
	///
	/// Print the status information for each track, if any.
	///
	/// ## Errors
	///
	/// This will return an error if there are I/O problems or the user aborts.
	pub fn status(&self, opts: &RipOptions, progress: &Progless, killed: KillSwitch)
	-> Result<(), RipRipError> {
		// Load the ripper.
		let mut rip = Ripper::new(self, opts)?;
		rip.status(progress, killed)?;
		rip.summarize_status();

		Ok(())
	}
}



/// # Format AccurateRip.
fn fmt_ar(ar: Option<(u8, u8)>, color: bool) -> Cow<'static, str> {
	if let Some((v1, v2)) = ar {
		let c1 =
			if ! color { "" }
			else if v1 == 0 { csi!(reset, light_red) }
			else if v1 <= 5 { csi!(reset, light_yellow) }
			else { csi!(reset, light_green) };

		let r1 =
			if color { csi!(reset, dim) }
			else { "" };

		let c2 =
			if ! color { "" }
			else if v2 == 0 { csi!(reset, light_red) }
			else if v2 <= 5 { csi!(reset, light_yellow) }
			else { csi!(reset, light_green) };

		let r2 =
			if color { csi!() }
			else { "" };

		Cow::Owned(format!(
			"        {c1}{:02}{r1}+{c2}{:02}{r2}",
			v1.min(99),
			v2.min(99),
		))
	}
	else { Cow::Borrowed("             ") }
}

#[expect(clippy::option_if_let_else, reason = "Too messy.")]
/// # Format CUETools.
fn fmt_ctdb(ctdb: Option<u16>, color: bool) -> Cow<'static, str> {
	if let Some(v1) = ctdb {
		let c1 =
			if ! color { "" }
			else if v1 == 0 { csi!(reset, light_red) }
			else if v1 <= 5 { csi!(reset, light_yellow) }
			else { csi!(reset, light_green) };

		let r1 =
			if color { csi!() }
			else { "" };

		Cow::Owned(format!(
			"       {c1}{:03}{r1}",
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
		let Some((dst, _, _)) = ripped.get(&track.number()) else {
			log!(@debug "Missing track {}; skipping cuesheet.", track.number());
			return None;
		};
		let Some(dst) = dst.file_name().and_then(OsStr::to_str) else {
			log!(
				@trace
				"Unable to obtain output file name for track {}; skipping cuesheet.",
				track.number(),
			);
			return None;
		};
		all.push((track, dst));
	}

	// The output folder.
	let Some(parent) = ripped.get(&1).and_then(|(dst, _, _)| dst.parent()) else {
		log!(@trace "Failed to find parent directory of first track; skipping cuesheet.");
		return None;
	};

	let mut cue = String::new();
	for (track, src) in all {
		// If there's an HTOA, it needs to be grouped with the first track.
		if track.position().is_first() && toc.htoa().is_some() {
			// This should have been ripped with everything else.
			let Some(src0) = ripped.get(&0)
				.and_then(|(dst, _, _)| dst.file_name())
				.and_then(OsStr::to_str) else {
				log!(@trace "Unable to obtain output file name for HTOA; skipping cuesheet.");
				return None;
			};

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
	let dst = parent.join(format!("{}.cue", cache_prefix(toc)));
	{
		use std::io::Write;
		let mut writer = CacheWriter::new(&dst).ok()?;
		writer.writer().write_all(cue.as_bytes()).ok()?;
		writer.finish().ok()?;
	}

	// Return the path.
	Some(dst)
}

/// # Write HTOA (Likely).
///
/// This writes the footnote explaining that the HTOA can't be verified but
/// ranks likely, which is as good as can be.
fn write_htoa_likely(stderr: &mut StderrLock<'static>) {
	use std::io::Write;

	// Always write to STDERR.
	let _res = writeln!(
		stderr,
		concat!(
			csi!(light_yellow), "\n*",
			csi!(reset, dim),
			" HTOA tracks cannot be verified w/ AccurateRip or CTDB,\n",
			"  but this rip rates ",
			csi!(reset, light_yellow), "likely",
			ansi!((reset, dim) ", which is the next best thing!"),
		),
	);
}

/// # Write HTOA (Any).
///
/// This writes the footnote explaining that HTOA can't be verified and the
/// ripped version sucks and should be improved.
fn write_htoa_any(stderr: &mut StderrLock<'static>) {
	use std::io::Write;

	// Always write to STDERR.
	let _res = writeln!(
		stderr,
		concat!(
			csi!(light_yellow), "\n*",
			csi!(reset, dim),
			" HTOA tracks cannot be verified w/ AccurateRip or CTDB\n",
			"  so you should re-rip it until it rates ",
			csi!(reset, light_yellow), "likely",
			ansi!((reset, dim) " to be safe."),
		),
	);
}



/// # Loggable AccurateRip.
struct LoggableAccurateRip(Option<(u8, u8)>);

impl fmt::Display for LoggableAccurateRip {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		if let Some((v1, v2)) = self.0 && (v1 != 0 || v2 != 0) {
			write!(f, "\n    AccurateRip {}+{}", v1.min(99), v2.min(99))
		}
		else { Ok(()) }
	}
}

/// # Loggable CTDB.
struct LoggableCTDB(Option<u16>);

impl fmt::Display for LoggableCTDB {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		if let Some(v) = self.0 && v != 0 {
			write!(f, "\n    CUETools {}", v.min(999))
		}
		else { Ok(()) }
	}
}

/// # Loggable Fruits.
struct LoggableFruits<'a>(&'a SavedRips);

impl fmt::Display for LoggableFruits<'_> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		for (idx, (file, ar, ctdb)) in self.0 {
			if *idx == 0 { write!(f, "\n  {} (HTOA)", file.display())?; }
			else {
				write!(
					f,
					"\n  {}{}{}",
					file.display(),
					LoggableAccurateRip(*ar),
					LoggableCTDB(*ctdb)
				)?;
			}
		}
		Ok(())
	}
}
