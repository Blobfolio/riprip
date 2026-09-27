/*!
# Rip Rip Hooray: Rip Manifests
*/

use cdtoc::{
	Toc,
	Track,
};
use crate::{
	cache_prefix,
	CacheWriter,
	cdtext::{
		CDText,
		DiscField,
		TrackField,
	},
	Isrc,
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

	/*
	/// # CDTOC.
	toc: &'a Toc,
	*/

	/// # HTOA.
	htoa: Option<(Track, &'a str)>,

	/// # Tracks.
	tracks: Vec<(Track, &'a str)>,

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
			//toc,
			htoa,
			tracks,
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
		let isrcs = self.cdtext().and_then(|v| v.isrcs())?;
		isrcs.get(&idx).copied()
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
	/// is technically fallible, so maybe?
	fn generate_cue(&self) -> Option<String> {
		use std::fmt::Write;

		// It might wind up bigger or smaller, but 2KiB is a good place to
		// start.
		let mut out = String::with_capacity(2048);

		// If there's CD-Text, start with some disc-level details.
		if let Some((cdtext_values, cdtext_bin)) = self.cdtext {
			// Barcode goes first, if we've got one. Note this is called
			// "CATALOG" in cuesheets.
			if let Some(barcode) = cdtext_values.barcode() {
				writeln!(&mut out, "CATALOG {barcode:#}").ok()?;
			}

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
}



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
