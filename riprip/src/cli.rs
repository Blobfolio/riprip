/*!
# Rip Rip Hooray: CLI
*/

use dactyl::traits::{
	BytesToUnsigned,
	NiceInflection,
};
use riprip_core::{
	CD_LEADOUT_LABEL,
	cdtoc::TocKind,
	Disc,
	DriveVendorModel,
	LogLog,
	macros::log,
	ReadOffset,
	RipOptions,
	RipRipError,
};
use std::fmt;



/// # Options Return Type.
///
/// This is awful, but not quite awful enough to warrant a struct. Haha.
pub(super) type Parsed = (
	RipOptions,
	Disc,
	Option<DriveVendorModel>,
	bool,
	bool,
	bool,
);



#[expect(clippy::too_many_lines, reason = "There's a lot to parse.")]
/// # Parse Options.
pub(super) fn parse() -> Result<Parsed, RipRipError> {
	argyle::argue! {
		Backward       "--backward"    "--backwards",
		FlipFlop       "--flip-flop",
		Help      "-h" "--help",
		NoCdText       "--no-cdtext",
		NoResume       "--no-resume",
		NoRip          "--no-rip",
		NoSummary      "--no-summary",
		Reset          "--reset",
		Status         "--status",
		Strict         "--strict",
		Sync           "--sync",
		Verbose   "-v" "--verbose",
		Version   "-V" "--version",

		@options
		Cache     "-c" "--cache",
		Device    "-d" "--dev",
		Confidence     "--confidence",
		Offset    "-o" "--offset",
		Passes    "-p" "--pass"        "--passes",
		ReRead    "-r" "--reread"      "--rereads",
		Tracks    "-t" "--track"       "--tracks",
	}

	let mut opts = RipOptions::default();
	let mut no_cdtext = false;
	let mut no_rip = false;
	let mut no_summary = false;
	let mut status = false;
	let mut cache = None;
	let mut dev = None;
	let mut offset = None;
	let mut tracks = String::new();
	let mut verbosity = 0_u8;
	for arg in Argument::args_os() {
		match arg {
			Argument::Backward =>  { opts = opts.with_backwards(true); },
			Argument::FlipFlop =>  { opts = opts.with_flip_flop(true); },
			Argument::NoCdText =>  { no_cdtext = true; },
			Argument::NoResume =>  { opts = opts.with_resume(false); },
			Argument::NoRip =>     { no_rip = true; },
			Argument::NoSummary => { no_summary = true; },
			Argument::Reset =>     { opts = opts.with_reset(true); },
			Argument::Status =>    { status = true; },
			Argument::Strict =>    { opts = opts.with_strict(true); },
			Argument::Sync =>      { opts = opts.with_sync(true); },
			Argument::Verbose =>   {
				verbosity += 1;
				opts = opts.with_verbose(true);
			},

			Argument::Help =>    return Err(RipRipError::PrintHelp),
			Argument::Version => return Err(RipRipError::PrintVersion),

			Argument::Cache(s) => {
				let s = parse_rip_option_cache(s)?;
				cache.replace(s);
			},
			Argument::Confidence(s) => {
				let s = u8::btou(s.trim().as_bytes())
					.ok_or(RipRipError::CliParse("--confidence"))?;
				opts = opts.with_confidence(s);
			},
			Argument::Device(s) => { dev.replace(s); },
			Argument::Offset(s) => {
				let s = ReadOffset::try_from(s.trim().as_bytes())
					.map_err(|_| RipRipError::CliParse("-o/--offset"))?;
				offset.replace(s);
			},
			Argument::Passes(s) => {
				let s = u8::btou(s.trim().as_bytes())
					.ok_or(RipRipError::CliParse("-p/--passes"))?;
				opts = opts.with_passes(s);
			},
			Argument::ReRead(s) => {
				let (a, b) = parse_rip_option_reread(s.as_bytes())?;
				opts = opts.with_rereads(a, b);
			},
			Argument::Tracks(s) => {
				if ! tracks.is_empty() { tracks.push(','); }
				tracks.push_str(&s);
			},

			Argument::Other(s) => return Err(RipRipError::CliArg(s)),
			Argument::OtherOs(s) => return Err(RipRipError::CliArg(s.to_string_lossy().into_owned())),
		}
	}

	// Enable logging as early as possible.
	let level = LogLog::init(verbosity);
	if level.is_some() {
		log!(@info "{}", RipRipError::PrintVersion);
	}

	// Figure out the disc and drive.
	let disc = Disc::new(dev, ! no_cdtext)?;
	let drivevendormodel = disc.drive_vendor_model();
	if level.is_some() {
		if let Some(v) = drivevendormodel { log!(@info "{v}"); }
		log!(@info "{}", LoggableDisc(&disc));
		log!(@info "{}", LoggableTracks(&disc));
		if let Some(cdtext) = disc.cdtext() {
			log!(@debug "Found CD-Text.{cdtext:#}");
		}
	}

	// Set up some drive-dependent things.
	if let Some(v) = cache.or_else(|| drivevendormodel.and_then(|vm| vm.detect_cache())) {
		opts = opts.with_cache(v);
	}
	if let Some(v) = offset.or_else(|| drivevendormodel.and_then(|vm| vm.detect_offset())) {
		opts = opts.with_offset(v);
	}

	// If we just want the status or didn't receive any -t, add everything.
	if status || tracks.is_empty() {
		let toc = disc.toc();
		if toc.htoa().is_some() { opts = opts.with_track(0); }
		for t in toc.audio_tracks() { opts = opts.with_track(t.number()); }
	}
	// Otherwise parse what we gathered earlier.
	else { opts = parse_rip_option_tracks(&disc, opts, &tracks)?; }

	Ok((
		opts,
		disc,
		drivevendormodel,
		no_rip,
		no_summary,
		status,
	))
}



/// # Parse Cache Size.
fn parse_rip_option_cache(cache: String) -> Result<u16, RipRipError> {
	let cache = cache.into_bytes();
	cache.iter()
		.position(|&b| matches!(b, b'm' | b'M'))
		.map_or_else(
			|| u16::btou(cache.trim_ascii()),
			|pos| u16::btou(cache[..pos].trim_ascii()).and_then(|v| v.checked_mul(1024)),
		)
		.ok_or(RipRipError::CliParse("-c/--cache"))
}

/// # Parse Re-read Option.
fn parse_rip_option_reread(v: &[u8]) -> Result<(u8, u8), RipRipError> {
	// Default.
	let mut a = 2;
	let mut b = 2;

	// If there's a comma, there could be up to two values. Keep the
	// default if either is omitted.
	let v = v.trim_ascii();
	// TODO: use split_once once stable.
	if let Some(pos) = v.iter().position(|b| b','.eq(b)) {
		let tmp = &v[..pos];
		if ! tmp.is_empty() {
			a = u8::btou(tmp).ok_or(RipRipError::CliParse("-r/--rereads"))?;
		}
		let tmp = &v[pos + 1..];
		if ! tmp.is_empty() {
			b = u8::btou(tmp).ok_or(RipRipError::CliParse("-r/--rereads"))?;
		}
	}
	// A number by itself affects only the first part.
	else {
		a = u8::btou(v).ok_or(RipRipError::CliParse("-r/--rereads"))?;
	}

	Ok((a, b))
}

/// # Parse Rip Tracks.
fn parse_rip_option_tracks(disc: &Disc, mut opts: RipOptions, tracks: &str)
-> Result<RipOptions, RipRipError> {
	for v in tracks.split(',') {
		let v = v.as_bytes().trim_ascii();
		if v.is_empty() { continue; }

		// It might be a range.
		// TODO: use split_once once stable.
		if let Some(pos) = v.iter().position(|b| b'-'.eq(b)) {
			// Split.
			let a = v[..pos].trim_ascii();
			let b = v[pos + 1..].trim_ascii();
			if a.is_empty() || b.is_empty() {
				return Err(RipRipError::CliParse("-t/--tracks"));
			}

			// Decode.
			let a = u8::btou(a).ok_or(RipRipError::CliParse("-t/--tracks"))?;
			let b = u8::btou(b).ok_or(RipRipError::CliParse("-t/--tracks"))?;

			// Add them all!
			if a <= b {
				for idx in a..=b { opts = opts.with_track(idx); }
			}
			else { return Err(RipRipError::CliParse("-t/--tracks")); }
		}
		// Otherwise it should be a single index.
		else {
			let v = u8::btou(v).ok_or(RipRipError::CliParse("-t/--tracks"))?;
			opts = opts.with_track(v);
		}
	}

	// Make sure the desired tracks are actually on the disc.
	let toc = disc.toc();
	if opts.has_tracks() {
		for idx in opts.tracks() {
			// Make sure the track is valid.
			let good =
				if idx == 0 { toc.htoa().is_some() }
				else { toc.audio_track(usize::from(idx)).is_some() };
			if ! good { return Err(RipRipError::NoTrack(idx)); }
		}
	}
	// If no tracks were specified, DO IT ALL.
	else {
		if toc.htoa().is_some() { opts = opts.with_track(0); }
		for t in toc.audio_tracks() { opts = opts.with_track(t.number()); }
	}

	Ok(opts)
}



/// # Loggable CD-Text.
///
/// This struct is used to format the CD-Text data in a format suitable for the
/// logger.
struct LoggableDisc<'a>(&'a Disc);

impl fmt::Display for LoggableDisc<'_> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let toc = self.0.toc();

		let kind = match toc.kind() {
			TocKind::Audio => "audio CD",
			TocKind::CDExtra => "CD-Extra",
			TocKind::DataFirst => "mixed data/audio CD",
		};

		write!(
			f,
			"Found {kind}.\n  CDTOC:       {toc}\n  AccurateRip: {ar}\n  CDDB:        {cddb}\n  CUETools:    {ctdb}\n  MusicBrainz: {mb}",
			ar=toc.accuraterip_id(),
			cddb=toc.cddb_id(),
			ctdb=toc.ctdb_id(),
			mb=toc.musicbrainz_id(),
		)?;

		// If we have a barcode, print that too.
		self.0.barcode().map_or(
			Ok(()),
			|barcode| write!(f, "\n  Barcode:     {barcode}")
		)
	}
}

/// # Loggable Track Listing.
///
/// This struct is used to format track-related output for the `-v`/`--verbose`
/// log.
struct LoggableTracks<'a>(&'a Disc);

impl fmt::Display for LoggableTracks<'_> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let toc = self.0.toc();

		// Start with the number of tracks.
		write!(
			f,
			"Found {}.",
			toc.audio_len().nice_inflect("audio track", "audio tracks"),
		)?;

		// Table of contents.
		let mut total = 0;
		write!(
			f,
			"\n  NO   FIRST    LAST  LENGTH          {}",
			if self.0.has_isrcs() { "ISRC" } else { "" },
		)?;

		// HTOA.
		if let Some(t) = toc.htoa() {
			let rng = t.sector_range_normalized();
			let len = rng.end - rng.start;
			write!(
				f,
				"\n  00  {:>6}  {:>6}  {:>6}          HTOA",
				rng.start,
				rng.end - 1,
				len,
			)?;
		}
		// Leading data track.
		else if matches!(toc.kind(), TocKind::DataFirst) {
			total += 1;
			write!(
				f,
				"\n  {:02}  {:>6}                    DATA TRACK",
				total,
				toc.data_sector_normalized().unwrap_or_default(),
			)?;
		}

		// The audio tracks.
		for t in toc.audio_tracks() {
			total += 1;
			let num = t.number();
			let rng = t.sector_range_normalized();
			let len = rng.end - rng.start;
			let isrc = self.0.isrc(num).unwrap_or_default();
			write!(
				f,
				"\n  {num:02}  {:>6}  {:>6}  {len:>6}  {isrc:>12}",
				rng.start,
				rng.end - 1,
			)?;
		}

		// Trailing data track.
		if matches!(toc.kind(), TocKind::CDExtra) {
			total += 1;
			write!(
				f,
				"\n  {:02}  {:>6}                    DATA TRACK",
				total,
				toc.data_sector_normalized().unwrap_or_default(),
			)?;
		}

		// The leadout.
		write!(
			f,
			"\n  {}  {:>6}                      LEAD-OUT",
			CD_LEADOUT_LABEL,
			toc.leadout_normalized(),
		)
	}
}
