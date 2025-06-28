/*!
# Rip Rip Hooray: Disc
*/

use cdtoc::{
	Toc,
	TocKind,
};
use crate::{
	Barcode,
	cache_prefix,
	CacheWriter,
	CD_LEADOUT_LABEL,
	CDTextKind,
	DriveVendorModel,
	KillSwitch,
	LibcdioInstance,
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
	Progless,
};
use std::{
	borrow::Cow,
	collections::HashMap,
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
	/// # CDIO Instance.
	cdio: LibcdioInstance,

	/// # Disc Table of Contents.
	toc: Toc,

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
			dim!("\n##   FIRST    LAST  LENGTH          {}\n"),
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
		let mut isrcs = HashMap::with_hasher(NoHash::default());
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
	/// # Barcode.
	pub const fn barcode(&self) -> Option<Barcode> { self.barcode }

	#[must_use]
	#[inline]
	/// # Drive Vendor and Model.
	pub fn drive_vendor_model(&self) -> Option<DriveVendorModel> {
		self.cdio.drive_vendor_model()
	}

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
			let writer = std::io::stderr();
			let mut handle = writer.lock();
			let mut total = 0;
			let mut good = 0;

			let htoa_any = saved.contains_key(&0);
			let htoa_likely = saved.get(&0).is_some_and(|(_, ar, ctdb)| ar.is_some() || ctdb.is_some());
			let conf = saved.values().any(|(_, ar, ctdb)| ar.is_some() || ctdb.is_some());
			let col1 = saved.first_key_value().map_or(0, |(_, (dst, _, _))| dst.to_string_lossy().len());

			let _res = writeln!(&mut handle, "\nThe fruits of your labor:");

			// If we did all tracks, make a cue sheet.
			if let Some(file) = save_cuesheet(&self.toc, &saved) {
				let _res = writeln!(&mut handle, dim!("  {}"), file.display());
			}

			for (idx, (file, ar, ctdb)) in saved {
				total += 1;
				if ar.is_some() || ctdb.is_some() { good += 1; }

				let _res = writeln!(
					&mut handle,
					concat!(dim!("  {:<col1$}"), "{}{}"),
					file.display(),
					if conf {
						if idx == 0 { Cow::Borrowed(ansi!((reset, light_yellow) "            *")) }
						else { fmt_ar(ar) }
					} else { Cow::Borrowed(ansi!((reset, light_red) "            x")) },
					if conf {
						if idx == 0 { Cow::Borrowed(ansi!((reset, light_yellow) "         *")) }
						else { fmt_ctdb(ctdb) }
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

			// Mention that the HTOA can't be verified but is probably okay.
			if htoa_likely {
				let _res = writeln!(
					&mut handle,
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
			// Mention that the HTOA can't be verified and should be reripped
			// to increase certainty.
			else if htoa_any {
				let _res = writeln!(
					&mut handle,
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

			// An extra line break for separation.
			let _res = writeln!(&mut handle).and_then(|()| handle.flush());
		}

		Ok(())
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
fn fmt_ar(ar: Option<(u8, u8)>) -> Cow<'static, str> {
	if let Some((v1, v2)) = ar {
		let c1 =
			if v1 == 0 { csi!(reset, light_red) }
			else if v1 <= 5 { csi!(reset, light_yellow) }
			else { csi!(reset, light_green) };

		let c2 =
			if v2 == 0 { csi!(reset, light_red) }
			else if v2 <= 5 { csi!(reset, light_yellow) }
			else { csi!(reset, light_green) };

		Cow::Owned(format!(
			concat!("        {c1}{:02}", csi!(reset, dim), "+{c2}{:02}", csi!()),
			v1.min(99),
			v2.min(99),
			c1=c1,
			c2=c2,
		))
	}
	else { Cow::Borrowed("             ") }
}

#[expect(clippy::option_if_let_else, reason = "Too messy.")]
/// # Format CUETools.
fn fmt_ctdb(ctdb: Option<u16>) -> Cow<'static, str> {
	if let Some(v1) = ctdb {
		let c1 =
			if v1 == 0 { csi!(reset, light_red) }
			else if v1 <= 5 { csi!(reset, light_yellow) }
			else { csi!(reset, light_green) };

		Cow::Owned(format!(
			concat!("       {c1}{:03}", csi!()),
			v1.min(999),
			c1=c1,
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
