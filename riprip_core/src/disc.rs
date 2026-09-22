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
	CD_LEADOUT,
	CddaDriver,
	CddaDriverExt,
	CddaDriverNewExt,
	cdtext::{
		CDText,
		CDTextError,
	},
	DriveVendorModel,
	Isrc,
	IsrcMap,
	KillSwitch,
	macros::log,
	RipOptions,
	Ripper,
	RipRipError,
	SavedRips,
};
use dactyl::traits::NiceInflection;
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
}

impl fmt::Debug for Disc {
	/// # Summarize the Disc.
	///
	/// This prints the various disc identifiers.
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		/// # Helper: Writing.
		///
		/// The on-screen and log summaries have (mostly) identical details,
		/// the formatting is just a little different.
		macro_rules! wrt {
			( $color:tt $k:literal $v:expr ) => (
				if f.alternate() {
					write!(
						f,
						"\n  {k:<12} {v}",
						k=$k,
						v=$v,
					)
				}
				else {
					writeln!(
						f,
						concat!(csi!(bold, $color), "{k:<12}", csi!(), " {v}"),
						k=$k,
						v=$v,
					)
				}
			);
		}

		// When logging, start with a count.
		if f.alternate() {
			write!(
				f,
				"Found {}.",
				match self.toc.kind() {
					TocKind::Audio => "audio CD",
					TocKind::CDExtra => "CD-Extra",
					TocKind::DataFirst => "mixed data/audio CD",
				},
			)?;
		}

		wrt!(199  "CDTOC:"       self.toc.to_string())?;
		wrt!(blue "AccurateRip:" self.toc.accuraterip_id())?;
		wrt!(blue "CDDB:"        cache_prefix(&self.toc))?;
		wrt!(blue "CUETools:"    self.toc.ctdb_id())?;
		wrt!(blue "MusicBrainz:" self.toc.musicbrainz_id())?;

		self.barcode().map_or(
			Ok(()),
			|barcode| wrt!(199 "Barcode:" barcode)
		)
	}
}

impl fmt::Display for Disc {
	/// # Summarize the Tracks.
	///
	/// This prints a table of contents summary of the disc.
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		/// # Helper: Writing.
		///
		/// The on-screen and log summaries have (mostly) identical details,
		/// the formatting is just a little different.
		macro_rules! wrt {
			( @dim $fmt:expr, $($tt:tt )+ ) => (
				if f.alternate() { write!(f, concat!("\n  ", $fmt), $( $tt )+) }
				else             { writeln!(f, dim!($fmt), $( $tt )+) }
			);
			( $fmt:expr, $($tt:tt )+ ) => (
				if f.alternate() { write!(f, concat!("\n  ", $fmt), $( $tt )+) }
				else             { writeln!(f, $fmt, $( $tt )+) }
			);
		}

		// When logging, start with a count.
		if f.alternate() {
			write!(
				f,
				"Found {}.",
				self.toc().audio_len().nice_inflect("audio track", "audio tracks")
			)?;
		}

		// Header.
		let (col_max, divider) = self.summary_colsize();
		let isrcs = self.isrcs();
		wrt!(
			@dim "NO   FIRST    LAST  LENGTH  {label:>col_max$}",
			label=if isrcs.is_some() { "ISRC" } else { "" },
			col_max=col_max,
		)?;
		wrt!(@dim "{divider}", divider=divider)?;

		// Keep track of the tracks.
		let mut total = 0;

		// HTOA.
		if let Some(t) = self.toc.htoa() {
			let rng = t.sector_range_normalized();
			wrt!(
				@dim "00  {start:>6}  {end:>6}  {len:>6}  {label:>col_max$}",
				start=rng.start,
				end=rng.end - 1,
				len=rng.end - rng.start,
				label="HTOA",
				col_max=col_max,
			)?;
		}
		// Leading data track.
		else if matches!(self.toc.kind(), TocKind::DataFirst) {
			total += 1;
			wrt!(
				@dim "{track:02}  {start:>6}                  {label:>col_max$}",
				track=total,
				start=self.toc.data_sector_normalized().unwrap_or_default(),
				label="DATA TRACK",
				col_max=col_max,
			)?;
		}

		// The audio tracks.
		for t in self.toc.audio_tracks() {
			total += 1;
			let num = t.number();
			let rng = t.sector_range_normalized();
			wrt!(
				"{track:02}  {start:>6}  {end:>6}  {len:>6}  {isrc}",
				track=num,
				start=rng.start,
				end=rng.end - 1,
				len=rng.end - rng.start,
				isrc=MaybeIsrc(isrcs.and_then(|v| v.get(&num).copied())),
			)?;
		}

		// Trailing data track.
		if matches!(self.toc.kind(), TocKind::CDExtra) {
			total += 1;
			wrt!(
				@dim "{track:02}  {start:>6}                  {label:>col_max$}",
				track=total,
				start=self.toc.data_sector_normalized().unwrap_or_default(),
				label="DATA TRACK",
				col_max=col_max,
			)?;
		}

		// The leadout.
		wrt!(
			@dim "{track:02X}  {start:>6}                  {label:>col_max$}",
			track=CD_LEADOUT,
			start=self.toc.leadout_normalized(),
			label="LEAD-OUT",
			col_max=col_max,
		)?;

		// Close it off!
		if f.alternate() { Ok(()) }
		else { writeln!(f, dim!("{divider}\n"), divider=divider) }
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
		if let Some(v) = dev.as_ref() {
			log!(@debug "Device path {}.", v.as_ref().display());
		}
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
		};

		// Unless the user opted out of CD-Text parsing, let's handle that
		// now.
		if cdtext && let Some(raw_cdtext) = out.cdda.cdtext() {
			match CDText::from_bytes(&raw_cdtext) {
				Ok(cdtext) => {
					out.barcode = cdtext.barcode();
					out.cdtext.replace((raw_cdtext, Ok(cdtext)));
				},
				Err(e) => {
					out.cdtext.replace((raw_cdtext, Err(e)));
				},
			}
		}

		// Look for barcode in subchannel if we don't have it yet.
		if out.barcode.is_none() && let Some(barcode) = out.cdda.mcn_subchannel() {
			out.barcode.replace(barcode);
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
	/// # Track ISRCs.
	pub fn isrcs(&self) -> Option<&IsrcMap> {
		self.cdtext().and_then(CDText::isrcs)
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
			let mut col1 = saved.first_key_value().map_or(0, |(_, (dst, _, _))| dst.to_string_lossy().len());

			// A header of sorts.
			let _res = writeln!(&mut handle, "\nThe fruits of your labor:");
			if let Some((cdtext1, cdtext2)) = self.cdtext_paths() {
				if cdtext1.is_file() {
					let _res = writeln!(&mut handle, dim!("  {}"), cdtext1.display());
					log!(@info "Saved CD-Text (raw).\n  {}", cdtext1.display());
					col1 = usize::max(col1, cdtext1.to_string_lossy().len());
				}
				if cdtext2.is_file() {
					let _res = writeln!(&mut handle, dim!("  {}"), cdtext2.display());
					log!(@info "Saved CD-Text.\n  {}", cdtext2.display());
					col1 = usize::max(col1, cdtext2.to_string_lossy().len());
				}
			}
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

	/// # CD-Text Paths.
	fn cdtext_paths(&self) -> Option<(PathBuf, PathBuf)> {
		let prefix = cache_prefix(&self.toc);
		let bin = cache_path(format!("{prefix}.cdtext.bin")).ok()?;
		let txt = cache_path(format!("{prefix}.cdtext.txt")).ok()?;
		Some((bin, txt))
	}

	/// # Save CD-Text Data.
	///
	/// Try to save the raw and decoded CD-Text data to disk.
	pub fn save_cdtext(&self, progress: &Progless) {
		let Some((bin, txt)) = &self.cdtext else { return; };
		let Some((dst_bin, dst_txt)) = self.cdtext_paths() else { return; };

		// Save the raw binary data first, unless it already exists.
		if
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
					prefix=cache_prefix(&self.toc),
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

impl Disc {
	/// # Track Formatting Alignment.
	///
	/// The rightmost track summary column has a variable width depending on
	/// the disc. This method returns an appropriate column size and divider
	/// to use for it.
	fn summary_colsize(&self) -> (usize, &'static str) {
		// ISRCs have fifteen characters.
		if self.isrcs().is_some() {
			(15, "-------------------------------------------")
		}
		// Without a data track, the longest label is "LEAD-OUT".
		else if matches!(self.toc.kind(), TocKind::Audio) {
			(8,  "------------------------------------")
		}
		// Otherwise "DATA TRACK".
		else {
			(10, "--------------------------------------")
		}
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
				[dst]
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

/// # Maybe ISRC?
///
/// Print it if it exists, print nothing if not.
struct MaybeIsrc(Option<Isrc>);

impl fmt::Display for MaybeIsrc {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		self.0.map_or(
			Ok(()),
			|v| <Isrc as fmt::Display>::fmt(&v, f)
		)
	}
}
