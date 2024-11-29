/*!
# Rip Rip Hooray: CLI
*/

use argyle::Argument;
use dactyl::traits::BytesToUnsigned;
use riprip_core::{
	Disc,
	DriveVendorModel,
	ReadOffset,
	RipRipError,
	RipOptions,
};



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



/// # Parse Options.
pub(super) fn parse() -> Result<Parsed, RipRipError> {
	let args = argyle::args()
		.with_keywords(include!(concat!(env!("OUT_DIR"), "/argyle.rs")));

	let mut opts = RipOptions::default();
	let mut no_rip = false;
	let mut no_summary = false;
	let mut status = false;
	let mut cache = None;
	let mut dev = None;
	let mut offset = None;
	let mut tracks = String::new();
	for arg in args {
		match arg {
			Argument::Key("--backward" | "--backwards") => {
				opts = opts.with_backwards(true);
			},
			Argument::Key("--flip-flop") => {
				opts = opts.with_flip_flop(true);
			},
			Argument::Key("-h" | "--help") => return Err(RipRipError::PrintHelp),
			Argument::Key("--no-resume") => { opts = opts.with_resume(false); },
			Argument::Key("--no-rip") => { no_rip = true; },
			Argument::Key("--no-summary") => { no_summary = true; },
			Argument::Key("--reset") => { opts = opts.with_reset(true); },
			Argument::Key("--status") => { status = true; },
			Argument::Key("--strict") => { opts = opts.with_strict(true); },
			Argument::Key("--sync") => { opts = opts.with_sync(true); },
			Argument::Key("-v" | "--verbose") => { opts = opts.with_verbose(true); },
			Argument::Key("-V" | "--version") => return Err(RipRipError::PrintVersion),

			Argument::KeyWithValue("-c" | "--cache", s) => {
				let s = parse_rip_option_cache(s)?;
				cache.replace(s);
			},
			Argument::KeyWithValue("-d" | "--dev", s) => { dev.replace(s); },
			Argument::KeyWithValue("-o" | "--offset", s) => {
				let s = ReadOffset::try_from(s.trim().as_bytes())
					.map_err(|_| RipRipError::CliParse("-o/--offset"))?;
				offset.replace(s);
			},
			Argument::KeyWithValue("-p" | "--pass" | "--passes", s) => {
				let s = u8::btou(s.trim().as_bytes())
					.ok_or(RipRipError::CliParse("-p/--passes"))?;
				opts = opts.with_passes(s);
			},
			Argument::KeyWithValue("-r" | "--reread" | "--rereads", s) => {
				let (a, b) = parse_rip_option_reread(s.as_bytes())?;
				opts = opts.with_rereads(a, b);
			},
			Argument::KeyWithValue("-t" | "--track" | "--tracks", s) => {
				if ! tracks.is_empty() { tracks.push(','); }
				tracks.push_str(&s);
			},

			_ => {},
		}
	}

	// Figure out the disc and drive.
	let disc = Disc::new(dev)?;
	let drivevendormodel = disc.drive_vendor_model();

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
