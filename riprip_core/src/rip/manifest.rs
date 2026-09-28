/*!
# Rip Rip Hooray: Rip Manifests
*/

use cdtoc::{
	Toc,
	Track,
};
use crate::{
	Barcode,
	cache_prefix,
	CacheWriter,
	cdtext::{
		CDText,
		DiscField,
		TrackField,
	},
	Isrc,
	IsrcMap,
	macros::log,
	SavedRips,
};
use std::{
	ffi::OsStr,
	fmt,
	path::{
		Path,
		PathBuf,
	},
};



/// # Manifest Writer.
///
/// This struct is used to help write `.cue` and `.toc` manifests for a rip.
pub(crate) struct RipManifest<'a> {
	/// # Stub.
	///
	/// The common output path for manifests, minus extension.
	stub: PathBuf,

	/// # HTOA.
	htoa: Option<(Track, &'a str)>,

	/// # Tracks.
	tracks: Vec<(Track, &'a str)>,

	/// # Barcode.
	barcode: Option<Barcode>,

	/// # ISRCs.
	isrcs: Option<&'a IsrcMap>,

	/// # CD-Text.
	///
	/// This holds the CD-Text values, as well as the file name of the binary
	/// dump.
	cdtext: Option<(&'a CDText, &'a str)>,
}

impl<'a> RipManifest<'a> {
	#[must_use]
	/// # New.
	///
	/// Initialize a new writer instance (without writing anything). Returns
	/// `None` if the rip is incomplete.
	pub(crate) fn new(
		toc: &'a Toc,
		ripped: &'a SavedRips,
		barcode: Option<Barcode>,
		isrcs: Option<&'a IsrcMap>,
		cdtext: Option<(&'a Path, &'a CDText)>,
	) -> Option<Self> {
		// We'll get to these.
		let mut stub = None;
		let mut htoa = None;

		// Sort out the tracks first, making sure they're all accounted for.
		let mut tracks = Vec::with_capacity(ripped.len());
		for track in toc.audio_tracks() {
			// Make sure the track was ripped.
			if
				let Some((dst, _, _)) = ripped.get(&track.number()) &&
				dst.is_file() &&
				let Some(name) = dst.file_name().and_then(OsStr::to_str)
			{
				tracks.push((track, name));

				// Fill out the stub if we haven't yet.
				if stub.is_none() && let Some(parent) = dst.parent() {
					stub.replace(parent.join(cache_prefix(toc)));
				}
			}
			else {
				log!(@debug "Missing track {}; skipping rip manifests.", track.number());
				return None;
			}
		}

		// If the TOC includes an HTOA, pull its details too.
		if let Some(track) = toc.htoa() {
			// Make sure the track was ripped.
			if
				let Some((dst, _, _)) = ripped.get(&track.number()) &&
				dst.is_file() &&
				let Some(name) = dst.file_name().and_then(OsStr::to_str)
			{
				htoa.replace((track, name));
			}
			else {
				log!(@debug "Missing HTOA; skipping rip manifests.");
				return None;
			}
		}

		// If we're here, the tracks were complete, so we should have a stub.
		let Some(stub) = stub else {
			std::hint::cold_path();
			log!(@trace [tracks] "Failed to generate manifest stub.");
			return None;
		};

		// Good enough!
		Some(Self {
			stub,
			htoa,
			tracks,
			barcode,
			isrcs,
			cdtext: cdtext.and_then(|(k, v)|
				if k.is_file() {
					k.file_name().and_then(OsStr::to_str).map(|k| (v, k))
				}
				else { None }
			),
		})
	}

	#[must_use]
	/// # Save `.cue` Cuesheet.
	///
	/// Generate and save the cuesheet, returning the path if successful.
	pub(crate) fn save_cue(&self) -> Option<PathBuf> {
		self.generate_cue().and_then(|v| self.save_manifest(v.as_bytes(), "cue"))
	}

	#[must_use]
	/// # Save `.cue` Cuesheet.
	///
	/// Generate and save the cuesheet, returning the path if successful.
	pub(crate) fn save_toc(&self) -> Option<PathBuf> {
		self.generate_toc().and_then(|v| self.save_manifest(v.as_bytes(), "toc"))
	}
}

impl RipManifest<'_> {
	#[must_use]
	/// # CD-Text Values.
	const fn cdtext(&self) -> Option<&CDText> {
		if let Some((v, _)) = self.cdtext { Some(v) }
		else { None }
	}

	#[must_use]
	/// # Track ISRC.
	fn isrc(&self, idx: u8) -> Option<Isrc> {
		self.isrcs.and_then(|v| v.get(&idx).copied())
	}

	#[must_use]
	/// # Save Manifest.
	///
	/// Save the given manifest, returning its path if successful.
	fn save_manifest(&self, data: &[u8], ext: &'static str) -> Option<PathBuf> {
		use std::io::Write;

		let dst = self.stub.with_extension(ext);
		{
			let mut writer = CacheWriter::new(&dst).ok()?;
			writer.writer().write_all(data).ok()?;
			writer.finish().ok()?;
		}

		// Easy, right?
		Some(dst)
	}
}

impl RipManifest<'_> {
	#[must_use]
	/// # Generate `.cue`.
	///
	/// Generate and return a `.cue`. This shouldn't fail, but `fmt::Write`
	/// is used to populate the string, so you never know.
	fn generate_cue(&self) -> Option<String> {
		use std::fmt::Write;

		// It might wind up bigger or smaller, but 2KiB is a good place to
		// start.
		let mut out = String::with_capacity(2048);

		// Barcode goes first, if we've got one. Note this is called
		// "CATALOG" in cuesheets.
		if let Some(barcode) = self.barcode {
			writeln!(&mut out, "CATALOG {barcode:#}").ok()?;
		}

		// If there's CD-Text, start with some disc-level details.
		if let Some((cdtext_values, cdtext_bin)) = self.cdtext {
			// CDTEXTFILE is second.
			writeln!(&mut out, "CDTEXTFILE \"{cdtext_bin}\"").ok()?;

			// Now TITLE, PERFORMER, and SONGWRITER, if any.
			if let Some(title) = cdtext_values.disc(DiscField::Title) {
				writeln!(&mut out, "TITLE \"{}\"", CueValueEscape(title)).ok()?;
			}
			if let Some(performer) = cdtext_values.disc(DiscField::Performer) {
				writeln!(&mut out, "PERFORMER \"{}\"", CueValueEscape(performer)).ok()?;
			}
			if let Some(songwriter) = cdtext_values.disc(DiscField::Songwriter) {
				writeln!(&mut out, "SONGWRITER \"{}\"", CueValueEscape(songwriter)).ok()?;
			}

			// Give an extra line.
			out.push('\n');
		}

		// Now the tracks.
		for (track, src) in &self.tracks {
			// If there's an HTOA, it needs to be grouped with the first track.
			if track.position().is_first() && let Some((_, htoa)) = self.htoa {
				writeln!(&mut out, "FILE \"{htoa}\" WAVE").ok()?;
				out.push_str("  TRACK 01 AUDIO\n");
				out.push_str("    INDEX 00 00:00:00\n");
				writeln!(&mut out, "FILE \"{src}\" WAVE").ok()?;
			}
			else {
				writeln!(&mut out, "FILE \"{src}\" WAVE").ok()?;
				writeln!(&mut out, "  TRACK {:02} AUDIO", track.number()).ok()?;
			}

			// Inject CD-Text TITLE and PERFORMER if we have them.
			if let Some(cdtext) = self.cdtext() {
				if let Some(title) = cdtext.track(TrackField::Title, track.number()) {
					writeln!(&mut out, "    TITLE \"{}\"", CueValueEscape(title)).ok()?;
				}
				if let Some(performer) = cdtext.track(TrackField::Performer, track.number()) {
					writeln!(&mut out, "    PERFORMER \"{}\"", CueValueEscape(performer)).ok()?;
				}
			}

			// The index is all the way down here. Haha.
			out.push_str("    INDEX 01 00:00:00\n");

			// Inject CD-Text ISRC, if any.
			if let Some(isrc) = self.isrc(track.number()) {
				writeln!(&mut out, "    ISRC {isrc:#}").ok()?;
			}
		}

		Some(out)
	}

	#[must_use]
	/// # Generate `.toc`.
	///
	/// Generate and return a `.toc`. This shouldn't fail, but `fmt::Write`
	/// is used to populate the string, so you never know.
	fn generate_toc(&self) -> Option<String> {
		use std::fmt::Write;

		// It might wind up bigger or smaller, but 2KiB is a good place to
		// start.
		let mut out = String::with_capacity(2048);

		// Barcode goes first, if we've got one. Note this is called
		// "CATALOG" in cuesheets.
		if let Some(barcode) = self.barcode {
			writeln!(&mut out, "CATALOG \"{barcode:#}\"").ok()?;
		}

		// The type is always CD_DA because `.toc` can't represent
		// multi-session content in a single file anyway.
		out.push_str("CD_DA\n");

		// Four our purposes here, CD-Text needs to be per-block.
		let cdtext = self.cdtext().map(CDText::blocks);
		if let Some(blocks) = cdtext {
			out.push_str("\nCD_TEXT {\n");

			// Language map.
			out.push_str("  LANGUAGE_MAP {\n");
			for (i, block) in blocks.iter().enumerate() {
				writeln!(&mut out, "    {i}: {}", block.language_code()).ok()?;
			}
			out.push_str("  }\n");

			// Disc-level Fields.
			for (i, block) in blocks.iter().enumerate() {
				writeln!(&mut out, "  LANGUAGE {i} {{").ok()?;
				for key in DiscField::ALL {
					match key {
						// Prefer the master barcode.
						DiscField::Barcode if let Some(v) = self.barcode =>
							writeln!(&mut out, "    UPC_EAN \"{v:#}\""),

						// Genre is binary.
						DiscField::Genre => writeln!(
							&mut out,
							"    GENRE {}",
							TocValue::Binary(block.raw_genre().unwrap_or(&[]))
						),

						// Everything else is a string, empty or not.
						_ => writeln!(
							&mut out,
							"    {} {}",
							key.as_str_toc(),
							TocValue::String(block.disc(key).unwrap_or("")),
						),
					}.ok()?;
				}
				out.push_str("  }\n");
			}

			out.push_str("}\n");
		}

		// Now the tracks.
		for (track, src) in &self.tracks {
			writeln!(
				&mut out,
				"\n// {}Track {:02}.\nTRACK AUDIO\nCOPY\nNO PRE_EMPHASIS\nTWO_CHANNEL_AUDIO",
				if track.position().is_first() && self.htoa.is_some() { "HTOA and " } else { "" },
				track.number(),
			).ok()?;

			// ISRC. (This might be redundant if there's CD-Text, but cdrdao
			// prints it twice in such cases.)
			if let Some(v) = self.isrc(track.number()) {
				writeln!(&mut out, "ISRC \"{v:#}\"").ok()?;
			}

			// CD-Text?
			if let Some(blocks) = cdtext {
				out.push_str("CD_TEXT {\n");
				for (i, block) in blocks.iter().enumerate() {
					writeln!(&mut out, "  LANGUAGE {i} {{").ok()?;
					for key in TrackField::ALL {
						match key {
							// These values are stored outside the catalog.
							TrackField::Isrc if let Some(v) = self.isrc(track.number()) =>
								writeln!(&mut out, "    ISRC \"{v:#}\""),

							// Everything else is a string, empty or not.
							_ => writeln!(
								&mut out,
								"    {} {}",
								key.as_str_toc(),
								TocValue::String(block.track(key, track.number()).unwrap_or("")),
							),
						}.ok()?;
					}
					out.push_str("  }\n");
				}
				out.push_str("}\n");
			}

			// If there's an HTOA, it needs to be grouped with the first track.
			if track.position().is_first() && let Some((htoa_track, htoa_src)) = self.htoa {
				let msf = sectors_to_msf(htoa_track.sectors());
				writeln!(
					&mut out,
					"FILE \"{htoa_src}\" 0 {:02}:{:02}:{:02}\nSTART",
					msf.0, msf.1, msf.2,
				).ok()?;
			}

			// Track file.
			let msf = sectors_to_msf(track.sectors());
			writeln!(
				&mut out,
				"FILE \"{src}\" 0 {:02}:{:02}:{:02}",
				msf.0, msf.1, msf.2,
			).ok()?;
		}

		// Done!
		Some(out)
	}
}



#[derive(Debug, Clone, Copy)]
/// # Escape `.cue` Value.
///
/// This struct is used to ensure that special characters — `'"'` — in quoted
/// `.cue` values are properly escaped.
struct CueValueEscape<'a>(&'a str);

impl fmt::Display for CueValueEscape<'_> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		use std::fmt::Write;

		if self.0.contains('"') {
			for c in self.0.chars() {
				// Quotes are escaped by doubling the problem.
				if c == '"' { f.write_str("\"\"")?; }
				else { f.write_char(c)?; }
			}
			Ok(())
		}
		else {
			<str as fmt::Display>::fmt(self.0, f)
		}
	}
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
/// # TOC CD-Text Value.
///
/// This prints a CD-Text value formatted (and escaped) for inclusion in a
/// `.toc` cuesheet. Unlike `.cue` values, these are not necessarily strings
/// so output includes the wrapping quotes, if any.
enum TocValue<'a> {
	/// # Binary.
	Binary(&'a [u8]),

	/// # String.
	String(&'a str),
}

impl fmt::Display for TocValue<'_> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		use std::fmt::Write;

		match self {
			// Binary value.
			Self::Binary([ next, rest @ .. ]) => {
				write!(f, " {{ {next}")?;
				for next in rest {
					write!(f, ", {next}")?;
				}
				f.write_str(" }")
			},

			// Empty binary value.
			Self::Binary([]) => { f.write_str("{}") },

			// String needing escaping.
			Self::String(v) if v.contains(['\\', '"']) => {
				f.write_char('"')?;
				for c in v.chars() {
					match c {
						'"' => f.write_str(r#"\""#)?,
						'\\' => f.write_str(r"\\")?,
						_ => f.write_char(c)?,
					}
				}
				f.write_char('"')
			},

			// String not needing escaping.
			Self::String(v) => { write!(f, "\"{v}\"") },
		}
	}
}

#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
#[must_use]
/// # Sectors to MSF.
const fn sectors_to_msf(sectors: u32) -> (u32, u8, u8) {
	// 75 sectors per second.
	let mut s = sectors.wrapping_div(75);
	let f = sectors - s * 75;

	// 60 seconds per minute.
	let m = s.wrapping_div(60);
	s -= m * 60;

	(m, s as u8, f as u8)
}
