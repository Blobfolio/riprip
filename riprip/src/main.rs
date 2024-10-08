/*!
# Rip Rip Hooray!
*/

#![forbid(unsafe_code)]

#![deny(
	clippy::allow_attributes_without_reason,
	clippy::correctness,
	unreachable_pub,
)]

#![warn(
	clippy::complexity,
	clippy::nursery,
	clippy::pedantic,
	clippy::perf,
	clippy::style,

	clippy::allow_attributes,
	clippy::clone_on_ref_ptr,
	clippy::create_dir,
	clippy::filetype_is_file,
	clippy::format_push_string,
	clippy::get_unwrap,
	clippy::impl_trait_in_params,
	clippy::lossy_float_literal,
	clippy::missing_assert_message,
	clippy::missing_docs_in_private_items,
	clippy::needless_raw_strings,
	clippy::panic_in_result_fn,
	clippy::pub_without_shorthand,
	clippy::rest_pat_in_fully_bound_structs,
	clippy::semicolon_inside_block,
	clippy::str_to_string,
	clippy::string_to_string,
	clippy::todo,
	clippy::undocumented_unsafe_blocks,
	clippy::unneeded_field_pattern,
	clippy::unseparated_literal_suffix,
	clippy::unwrap_in_result,

	macro_use_extern_crate,
	missing_copy_implementations,
	missing_docs,
	non_ascii_idents,
	trivial_casts,
	trivial_numeric_casts,
	unused_crate_dependencies,
	unused_extern_crates,
	unused_import_braces,
)]



use argyle::{
	Argue,
	ArgyleError,
	FLAG_HELP,
	FLAG_VERSION,
};
use dactyl::{
	NiceU16,
	traits::BytesToUnsigned,
};
use fyi_msg::{
	Msg,
	Progless,
};
use riprip_core::{
	Disc,
	DriveVendorModel,
	KillSwitch,
	ReadOffset,
	RipRipError,
	RipOptions,
};
use std::{
	borrow::Cow,
	sync::{
		atomic::{
			AtomicBool,
			Ordering::{
				Relaxed,
				SeqCst,
			},
		},
		Arc,
	},
};
use utc2k::FmtUtc2k;



/// # A Divider Line.
///
/// This is used to encase the drive vendor/model during summary. We'll slice
/// it to match the length rather than `"-".repeat()` or whatever.
const DIVIDER: &str = "------------------------";



/// # Main.
///
/// This lets us bubble up startup errors so they can be pretty-printed.
fn main() {
	match _main() {
		Ok(()) => {},
		Err(RipRipError::Argue(ArgyleError::WantsVersion)) => {
			println!(concat!("Rip Rip Hooray! v", env!("CARGO_PKG_VERSION")));
		},
		Err(RipRipError::Argue(ArgyleError::WantsHelp)) => {
			helper();
		},
		Err(e) => {
			Msg::from(e).eprint();
			std::process::exit(1);
		},
	}
}

#[inline]
/// # Actual Main.
///
/// This does all the stuff.
fn _main() -> Result<(), RipRipError> {
	// Load CLI arguments, if any.
	let args = Argue::new(FLAG_HELP | FLAG_VERSION)?;

	// Check for unknown args.
	if let Some(boo) = args.check_keys(
		&[
			b"--backward",
			b"--backwards",
			b"--flip-flop",
			b"--no-resume",
			b"--no-rip",
			b"--no-summary",
			b"--reset",
			b"--status",
			b"--strict",
			b"--sync",
			b"--verbose",
			b"-v",
		],
		&[
			b"--cache",
			b"--confidence",
			b"--dev",
			b"--offset",
			b"--pass",
			b"--passes",
			b"--reread",
			b"--rereads",
			b"--track",
			b"--tracks",
			b"-c",
			b"-d",
			b"-o",
			b"-p",
			b"-r",
			b"-t",
		],
	) {
		return Err(RipRipError::CliArg(String::from_utf8_lossy(boo).into_owned()));
	}

	// Connect to the device and summarize the disc.
	let dev = args.option2_os(b"-d", b"--dev");
	let disc = Disc::new(dev)?;
	let drivevendormodel = disc.drive_vendor_model();

	// Quiet?
	if ! args.switch(b"--no-summary") {
		if let Some(vm) = drivevendormodel {
			let vm = vm.to_string();
			if ! vm.is_empty() {
				eprintln!(
					"\x1b[2;36m{}\n\x1b[0;1;36m{vm}\n\x1b[0;2;36m{}\n\x1b[0m",
					&DIVIDER[..vm.len()],
					&DIVIDER[..vm.len()],
				);
			}
		}

		eprintln!("{disc}");
	}

	// Go ahead and leave if there's no ripping to do.
	if args.switch(b"--no-rip") { return Ok(()); }

	// Set up progress and killswitch in case they're needed.
	let progress = Progless::default();
	let killed = KillSwitch::default();
	sigint(killed.inner(), Some(progress.clone()));

	// Just checking the status?
	if let Some(opts) = parse_rip_status_options(&args, &disc)? {
		return disc.status(&opts, &progress, &killed);
	}

	// Parse the options.
	let opts = parse_rip_options(&args, drivevendormodel, &disc)?;
	rip_summary(&disc, &opts)?;

	// Log header.
	if opts.verbose() { log_header(&disc, &opts); }

	// Rip and rip and rip!
	disc.rip(&opts, &progress, &killed)?;

	if killed.killed() { Err(RipRipError::Killed) }
	else { Ok(()) }
}

/// # Log Header.
///
/// Print a few basic setup details for the log. Only applies when -v/--verbose
/// is set, and we're ripping something.
fn log_header(disc: &Disc, opts: &RipOptions) {
	use std::io::Write;

	let writer = std::io::stdout();
	let mut handle = writer.lock();

	// Program version.
	let _res = writeln!(
		&mut handle,
		concat!("#####
## Rip Rip Hooray! v", env!("CARGO_PKG_VERSION"), "
## {}
##"),
		opts.cli(),
	);

	// Drive.
	if let Some(v) = disc.drive_vendor_model() {
		let vendor = v.vendor();
		let model = v.model();
		if vendor.is_empty() {
			let _res = writeln!(&mut handle, "## Drive: {model}");
		}
		else {
			let _res = writeln!(&mut handle, "## Drive: [{vendor}] {model}");
		}
	}

	// Everything else!
	let _res = writeln!(
		&mut handle,
		"## Disc:  {}
## Date:  {}
##
## The quality issues noted for each pass are composed of the following fields,
## separated by two spaces:
##   * Track Number                   [2 digits]
##   * Logical Sector Number          [6 digits]
##   * Affected Samples (out of 588)  [3 digits]
##   * Description
##       * BAD:      values returned with C2 errors
##       * CONFUSED: many contradictory \"good\" values
#####",
		FmtUtc2k::now(),
		disc.toc().cddb_id(),
	);

	let _res = handle.flush();
}

/// # Parse Rip Options.
fn parse_rip_options(args: &Argue, drive: Option<DriveVendorModel>, disc: &Disc) -> Result<RipOptions, RipRipError> {
	let mut opts = RipOptions::default()
		.with_backwards(args.switch2(b"--backwards", b"--backward"))
		.with_flip_flop(args.switch(b"--flip-flop"))
		.with_reset(args.switch(b"--reset"))
		.with_resume(! args.switch(b"--no-resume"))
		.with_strict(args.switch(b"--strict"))
		.with_sync(args.switch(b"--sync"))
		.with_verbose(args.switch2(b"-v", b"--verbose"));

	if let Some(v) = args.option2(b"-o", b"--offset") {
		let v = ReadOffset::try_from(v)
			.map_err(|_| RipRipError::CliParse("-o/--offset"))?;
		opts = opts.with_offset(v);
	}
	else if let Some(v) = drive.and_then(|vm| vm.detect_offset()) {
		opts = opts.with_offset(v);
	}

	// Cache is more annoying than some options, less annoying than others.
	if let Some(v) = args.option2(b"-c", b"--cache") {
		let v = v.iter()
			.position(|&b| matches!(b, b'm' | b'M'))
			.map_or_else(
				|| u16::btou(v),
				|pos| u16::btou(v[..pos].trim_ascii()).and_then(|v| v.checked_mul(1024))
			)
			.ok_or(RipRipError::CliParse("-c/--cache"))?;
		opts = opts.with_cache(v);
	}
	else if let Some(v) = drive.and_then(|vm| vm.detect_cache()) {
		opts = opts.with_cache(v);
	}

	if let Some(v) = parse_rip_option_confidence(args)? {
		opts = opts.with_confidence(v);
	}

	if let Some(v) = args.option2(b"-p", b"--passes").or_else(|| args.option(b"--pass")) {
		let passes = u8::btou(v).ok_or(RipRipError::CliParse("-p/--passes"))?;
		opts = opts.with_passes(passes);
	}

	if let Some((a, b)) = parse_rip_option_reread(args)? {
		opts = opts.with_rereads(a, b);
	}

	// Tracks are also kinda annoying.
	let toc = disc.toc();
	for v in args.option2_values(b"-t", b"--tracks", Some(b',')).chain(args.option_values(b"--track", Some(b','))) {
		let v = v.trim_ascii();
		if v.is_empty() { continue; }

		// It might be a range.
		if let Some(pos) = v.iter().position(|b| b'-'.eq(b)) {
			// Split.
			let a = v[..pos].trim_ascii();
			let b = v[pos + 1..].trim_ascii();
			if a.is_empty() || b.is_empty() { return Err(RipRipError::CliParse("-t/--tracks")); }

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
	if opts.has_tracks() {
		for idx in opts.tracks() {
			// Make sure the track is valid.
			let good =
				if idx == 0 { toc.htoa().is_some() }
				else { toc.audio_track(usize::from(idx)).is_some() };
			if ! good {
				return Err(RipRipError::NoTrack(idx));
			}
		}
	}
	// If no tracks were specified, DO IT ALL.
	else {
		if toc.htoa().is_some() { opts = opts.with_track(0); }
		for t in toc.audio_tracks() { opts = opts.with_track(t.number()); }
	}

	// Done!
	Ok(opts)
}

/// # Parse Rip (Status) Options.
///
/// Return (mostly) default options if `--status` is set.
fn parse_rip_status_options(args: &Argue, disc: &Disc)
-> Result<Option<RipOptions>, RipRipError> {
	if args.switch(b"--status") {
		// Make a generic options with all the tracks.
		let mut opts = RipOptions::default();

		if let Some(v) = parse_rip_option_confidence(args)? {
			opts = opts.with_confidence(v);
		}

		if let Some((a, b)) = parse_rip_option_reread(args)? {
			opts = opts.with_rereads(a, b);
		}

		// Add all tracks from the disc.
		let toc = disc.toc();
		if toc.htoa().is_some() { opts = opts.with_track(0); }
		for t in toc.audio_tracks() { opts = opts.with_track(t.number()); }

		Ok(Some(opts))
	}
	else { Ok(None) }
}

/// # Parse Confidence Option.
fn parse_rip_option_confidence(args: &Argue) -> Result<Option<u8>, RipRipError> {
	if let Some(v) = args.option(b"--confidence") {
		let confidence = u8::btou(v).ok_or(RipRipError::CliParse("--confidence"))?;
		Ok(Some(confidence))
	}
	else { Ok(None) }
}

/// # Parse Re-read Option.
fn parse_rip_option_reread(args: &Argue) -> Result<Option<(u8, u8)>, RipRipError> {
	// Rereads are kinda annoying.
	if let Some(v) = args.option2(b"-r", b"--rereads").or_else(|| args.option(b"--reread")) {
		// Default.
		let mut a = 2;
		let mut b = 2;

		// If there's a comma, there could be up to two values. Keep the
		// default if either is omitted.
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

		Ok(Some((a, b)))
	}
	else { Ok(None) }
}

/// # Rip Summary.
///
/// Summarize and confirm the chosen settings before proceeding.
fn rip_summary(disc: &Disc, opts: &RipOptions) -> Result<(), RipRipError> {
	// Build up all the messy values.
	let nice_c2 = Cow::Borrowed(
		if opts.strict() { "C2 Error Pointers \x1b[0;2m(\x1b[0;1;93mSector\x1b[0;2m)" }
		else { "C2 Error Pointers \x1b[0;2m(\x1b[0;1mSample\x1b[0;2m)" }
	);
	let nice_cache = opts.cache().map_or(
		Cow::Borrowed("Disabled"),
		|c| Cow::Owned(format!("{} KiB", NiceU16::from(c.get())))
	);
	let nice_chk = Cow::Owned(format!(
		"AccurateRip/CTDB cf. {}+",
		opts.confidence(),
	));
	let nice_offset = Cow::Owned(format!("{}", opts.offset().samples()));
	let nice_output = Cow::Owned(format!(
		"./{}/{}_\x1b[0;2m##\x1b[0;1m.wav",
		riprip_core::CACHE_BASE,
		disc.toc().cddb_id(),
	));
	let nice_passes = Cow::Owned(format!(
		"{}{}",
		opts.passes(),
		if opts.resume() {
			if opts.reset() { " \x1b[0;2m(\x1b[0;1;93mReset Counts\x1b[0;2m)" }
			else { "" }
		}
		else { " \x1b[0;2m(\x1b[0;1;93mFrom Scratch\x1b[0;2m)" },
	));
	let nice_read_order = Cow::Borrowed(
		if opts.flip_flop() { "Alternate" }
		else if opts.backwards() { "Backwards" }
		else { "Normal" }
	);
	let (rr_a, rr_b) = opts.rereads();
	let nice_rereads1 =
		if rr_a == 1 { Cow::Borrowed("Re-Read Consistency") }
		else { Cow::Owned(format!("Re-Read Consistency {rr_a}+")) };
	let nice_rereads2 =
		if rr_b == 1 { Cow::Borrowed("Re-Read Contention") }
		else { Cow::Owned(format!("Re-Read Contention {rr_b}×")) };
	let nice_sync = Cow::Borrowed("Subchannel Sync");
	let nice_tracks = Cow::Owned(rip_summary_tracks(opts));
	let nice_verbose = Cow::Borrowed(if opts.verbose() { "Yes" } else { "No" });

	// Combine the values with labels so we can at least somewhat cleanly
	// display everything. Haha.
	let set = [
		("Tracks:", nice_tracks, true),
		("Read Offset:", nice_offset, 0 != opts.offset().samples_abs()),
		("Cache Bust:", nice_cache, opts.cache().is_some()),
		("Verification:", nice_chk, true),
		("", nice_c2, true),
		("", nice_rereads1, 1 != rr_a),
		("", nice_rereads2, 1 != rr_b),
		("", nice_sync, opts.sync()),
		("Rip Passes:", nice_passes, true),
		("Read Order:", nice_read_order, true),
		("Verbose:", nice_verbose, opts.verbose()),
		("Destination:", nice_output, true),
	];
	let max_label = set.iter().map(|(k, _, _)| k.len()).max().unwrap_or(0);

	// Print them!
	eprintln!("\x1b[1;38;5;199mRip Rip…\x1b[0m");
	for (k, v, enabled) in set {
		if enabled {
			eprintln!("  {k:max_label$} \x1b[1m{v}\x1b[0m");
		}
		else if k.is_empty() {
			eprintln!("  \x1b[2m{k:max_label$} \x1b[9m{v}\x1b[0m");
		}
		else {
			eprintln!("  \x1b[2;9m{k:max_label$} {v}\x1b[0m");
		}
	}

	// One last chance to bail!
	if Msg::plain("\x1b[1;38;5;199m…Hooray?\x1b[0m").eprompt_with_default(true) {
		eprintln!("\n");
		Ok(())
	}
	else {
		eprintln!();
		Err(RipRipError::Killed)
	}
}

/// # Rip Summary Tracks.
///
/// Format the desired tracks into a compact string.
///
/// Note: this value assumes ASCII bold and clear codes will be appended to
/// either end prior to print.
fn rip_summary_tracks(opts: &RipOptions) -> String {
	use oxford_join::OxfordJoin;

	let mut set = opts.tracks_rng()
		.map(|rng| {
			let (a, b) = rng.into_inner();
			if a == b { a.to_string() }
			else { format!("{a}\x1b[0;2m..=\x1b[0;1m{b}") }
		})
		.collect::<Vec<_>>();

	match set.len() {
		1 => set.remove(0),
		2 => set.join("\x1b[0;2m and \x1b[0;1m"),
		_ => set.oxford_and()
			.replace(',', "\x1b[0;2m,\x1b[0;1m")
			.replace("\x1b[2m,\x1b[0;1m and ", "\x1b[0;2m, and \x1b[0;1m"),
	}
}

/// # Hook Up CTRL+C.
fn sigint(killed: Arc<AtomicBool>, progress: Option<Progless>) {
	let _res = ctrlc::set_handler(move ||
		if killed.compare_exchange(false, true, SeqCst, Relaxed).is_ok() {
			if let Some(p) = &progress { p.sigint(); }
		}
	);
}

#[cold]
/// # Print Help.
fn helper() {
	println!(concat!(
		r#"
    n__n_
   /  = =\     "#, "\x1b[38;5;199mRip Rip Hooray!\x1b[0;38;5;69m v", env!("CARGO_PKG_VERSION"), "\x1b[0m", r#"
  /   ._Y_)    Accurate, incremental audio
 /      "\     CD ripping and recovery.
(_/  (_,  \
  \      ( \_,--""""--.
 ,-`.___,-` )-.______.'
 `-,'   `-_-'

USAGE:
    riprip [OPTIONS]

BASIC SETTINGS:
    -r, --rereads <[ABS],[MUL]>
                      Re-read sectors on subsequent passes until A) they have
                      been independently verified with AccurateRip or CUETools;
                      or B) the same allegedly-good values have been read at
                      least <ABS> times, and <MUL> times more often than any
                      contradictory "good" values. The value may omit the
                      number on either side of the comma to keep the default,
                      or be a single number to alter only the <ABS>.
                      [default: 2,2; range: 1..=20,1..=10]
    -p, --passes <NUM>
                      Automate re-ripping by executing up to <NUM> passes for
                      each track while any samples remain unread or
                      unconfirmed. [default: 1; max: 16]
    -t, --tracks <NUM(s),RNG>
                      Rip one or more specific tracks (rather than the whole
                      disc). Multiple tracks can be separated by commas (2,3),
                      specified as an inclusive range (2-3), and/or given their
                      own -t/--track (-t 2 -t 3). Track 0 can be used to rip
                      the HTOA, if any. [default: the whole disc]

WHEN ALL ELSE FAILS:
        --backwards   Reverse the sector read order when ripping a track,
                      starting at end, and ending at the start.
        --flip-flop   Alternate the sector read order between passes, forwards
                      then backwards then forwards then backwards… This has no
                      effect unless -p/--passes is at least two.
        --no-resume   Ignore any previous rip states, starting over from
                      scratch.
        --reset       Flip "likely" samples back to "maybe", keeping their
                      values, but resetting all counts to one. This is a softer
                      alternative to --no-resume, and will not affect tracks
                      confirmed by AccurateRip/CUETools.
        --strict      Consider C2 errors an all-or-nothing proposition for the
                      sector as a whole, marking all samples bad if any of them
                      are bad. This is most effective when applied consistently
                      from the initial rip and onward.

DRIVE SETTINGS:
    -c, --cache <NUM> Drive cache can interfere with re-read accuracy. If your
                      drive caches data, use this option to specify its buffer
                      size so Rip Rip can try to mitigate it. Values with an
                      M suffix are treated as MiB, otherwise KiB are assumed.
                      [default: auto or 0; max: 65,535]
    -d, --dev <PATH>  The device path for the optical drive containing the CD
                      of interest, like /dev/cdrom. [default: auto]
    -o, --offset <SAMPLES>
                      The AccurateRip, et al, sample read offset to apply to
                      data retrieved from the drive.
                      [default: auto or 0; range: ±5880]

UNUSUAL SETTINGS:
        --confidence <NUM>
                      Consider a track accurately ripped — i.e. stop working on
                      it — AccurateRip and/or CUETools matches are found with a
                      confidence of at least <NUM>. Raise this value if you
                      personally fucked up the database(s) with prior bad rips,
                      otherwise the default should be fine. Haha.
                      [default: 3; range: 1..=10]
        --sync        Confirm sector positioning with subchannel data (when
                      available) to make sure the drive is actually reading
                      from the right place, and ignore the data if not. This is
                      prone to false-positives — subchannel data is easily
                      corrupted — so only recommended when disc rot, rather
                      than wear-and-tear, is the sole cause of your woes.

MISCELLANEOUS:
    -h, --help        Print help information to STDOUT and exit.
    -v, --verbose     Print detailed sector quality information to STDOUT, so
                      it can e.g. be piped to a file for review, like:
                      riprip -v > issues.log
    -V, --version     Print version information to STDOUT and exit.
        --no-rip      Print the basic drive and disc information to STDERR and
                      exit (without ripping anything).
        --no-summary  Skip the drive and disc summary and jump straight to
                      ripping.
        --status      Print the status of the individual track rips (that you
                      presumably already started) to STDERR and exit. Note that
                      only the --no-summary, --confidence, and -r/--rereads
                      options have any meaning in this mode.

EARLY EXIT:
    If you don't have time to let a rip finish naturally, press "#, "\x1b[38;5;208mCTRL\x1b[0m+\x1b[38;5;208mC\x1b[0m to stop
    it early. Your progress will still be saved, there just won't be as much of
    it. Haha.
"
	));
}
