/*!
# Rip Rip Hooray!
*/

#![forbid(unsafe_code)]

#![warn(
	clippy::filetype_is_file,
	clippy::integer_division,
	clippy::needless_borrow,
	clippy::nursery,
	clippy::pedantic,
	clippy::perf,
	clippy::suboptimal_flops,
	clippy::unneeded_field_pattern,
	macro_use_extern_crate,
	missing_copy_implementations,
	missing_debug_implementations,
	missing_docs,
	non_ascii_idents,
	trivial_casts,
	trivial_numeric_casts,
	unreachable_pub,
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
	NiceElapsed,
	NiceU8,
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
use trimothy::TrimSlice;



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

	// Set up the ripper!
	let opts = parse_rip_options(&args, drivevendormodel, &disc)?;
	let progress = Progless::default();
	let killed = KillSwitch::default();
	sigint(killed.inner(), Some(progress.clone()));

	// Summarize.
	rip_summary(&opts)?;

	// Rip and rip and rip!
	let now = std::time::Instant::now();
	disc.rip(&opts, &progress, &killed)?;

	eprintln!();
	if killed.killed() { Err(RipRipError::Killed) }
	else {
		Msg::success(format!("Finished in {}.", NiceElapsed::from(now))).eprint();
		Ok(())
	}
}

/// # Parse Rip Options.
fn parse_rip_options(args: &Argue, drive: Option<DriveVendorModel>, disc: &Disc) -> Result<RipOptions, RipRipError> {
	let mut opts = RipOptions::default()
		.with_backwards(args.switch(b"--backwards"))
		.with_c2(! args.switch(b"--no-c2"))
		.with_cache_bust(! args.switch(b"--no-cache-bust"))
		.with_raw(args.switch(b"--raw"))
		.with_resume(! args.switch(b"--no-resume"))
		.with_strict(args.switch(b"--strict"));

	if let Some(v) = args.option2(b"-o", b"--offset") {
		let v = ReadOffset::try_from(v)
			.map_err(|_| RipRipError::CliParse("-o/--offset"))?;
		opts = opts.with_offset(v);
	}
	else if let Some(v) = drive.and_then(|vm| vm.detect_offset()) {
		opts = opts.with_offset(v);
	}

	if let Some(v) = args.option(b"--confidence") {
		let confidence = u8::btou(v).ok_or(RipRipError::CliParse("--confidence"))?;
		opts = opts.with_cutoff(confidence);
	}

	if let Some(v) = args.option(b"--cutoff") {
		let cutoff = u8::btou(v).ok_or(RipRipError::CliParse("--cutoff"))?;
		opts = opts.with_cutoff(cutoff);
	}

	if let Some(v) = args.option2(b"-r", b"--refine") {
		let refine = u8::btou(v).ok_or(RipRipError::CliParse("-r/--refine"))?;
		opts = opts.with_refine(refine);
	}

	// Parsing the tracks is slightly more involved. Haha.
	for v in args.option2_values(b"-t", b"--track", Some(b',')) {
		let v = v.trim();
		if v.is_empty() { continue; }

		// It might be a range.
		if let Some(pos) = v.iter().position(|b| b'-'.eq(b)) {
			// Split.
			let a = v[..pos].trim();
			let b = v[pos + 1..].trim();
			if a.is_empty() || b.is_empty() { return Err(RipRipError::CliParse("-t/--track")); }

			// Decode.
			let a = u8::btou(a).ok_or(RipRipError::CliParse("-t/--track"))?;
			let b = u8::btou(b).ok_or(RipRipError::CliParse("-t/--track"))?;

			// Add them all!
			if a <= b {
				for idx in a..=b { opts = opts.with_track(idx); }
			}
			else { return Err(RipRipError::CliParse("-t/--track")); }
		}
		// Otherwise it should be a single index.
		else {
			let v = u8::btou(v).ok_or(RipRipError::CliParse("-t/--track"))?;
			opts = opts.with_track(v);
		}
	}

	// If we didn't parse any tracks, add each track on the disc.
	if ! opts.has_tracks() {
		// Include the HTOA if we're ripping everything.
		if disc.toc().htoa().is_some() { opts = opts.with_track(0); }
		for t in disc.toc().audio_tracks() { opts = opts.with_track(t.number()); }
	}

	// Done!
	Ok(opts)
}

/// # Rip Summary.
///
/// Summarize and confirm the chosen settings before proceeding.
fn rip_summary(opts: &RipOptions) -> Result<(), RipRipError> {
	use oxford_join::OxfordJoin;

	let nice_tracks = Cow::Owned({
		let mut last = u8::MAX;
		let mut continuous = true;
		let tracks = opts.tracks()
			.map(|n| {
				if last != u8::MAX && last + 1 != n { continuous = false; }
				last = n;
				NiceU8::from(n)
			})
			.collect::<Vec<NiceU8>>();

		let len = tracks.len();
		if len == 1 { tracks[0].to_string() }
		else if 2 < len && continuous {
			format!("{}\x1b[2m..=\x1b[0;1m{}", tracks[0], tracks[len - 1])
		}
		else {
			tracks.oxford_and()
				.replace(',', "\x1b[2m,\x1b[0;1m")
				.replace(" and ", "\x1b[2m and \x1b[0;1m")
		}
	});
	let nice_offset = Cow::Owned(format!("{}", opts.offset().samples()));
	let nice_output = Cow::Owned(format!(
		"./{}/##.{}",
		riprip_core::CACHE_BASE,
		if opts.raw() { "pcm" } else { "wav" },
	));
	let cutoff = opts.cutoff();
	let nice_verify = Cow::Owned(format!(
		"{}{}AccurateRip/CTDB ({})",
		match (opts.c2(), opts.strict()) {
			(true, true) => "(Strict) Sector C2\x1b[2m;\x1b[0;1m ",
			(true, false) => "Sample C2\x1b[2m;\x1b[0;1m ",
			_ => "",
		},
		if 1 < cutoff { format!("Re-Read ({})\x1b[2m;\x1b[0;1m ", cutoff - 1) } else { String::new() },
		opts.confidence(),
	));
	let nice_passes = NiceU8::from(opts.passes());

	let set = [
		("Tracks:", nice_tracks, true),
		("Read Offset:", nice_offset, 0 != opts.offset().samples_abs()),
		("Verification:", nice_verify, true),
		("Rip Passes:", Cow::Borrowed(nice_passes.as_str()), true),
		("Destination:", nice_output, true),
		("Backwards:", yesno(opts.backwards()), opts.backwards()),
		("Bust Cache:", yesno(opts.cache_bust()), opts.cache_bust()),
		("Resumable:", yesno(opts.resume()), opts.resume()),
	];
	let max_label = set.iter().map(|(k, _, _)| k.len()).max().unwrap_or(0);

	eprintln!("\x1b[1;38;5;199mRip Rip…\x1b[0m");
	for (k, v, enabled) in set {
		if enabled {
			eprintln!("  {k:max_label$} \x1b[1m{v}\x1b[0m");
		}
		else {
			eprintln!("  \x1b[2;9m{k:max_label$} {v}\x1b[0m");
		}
	}

	// One last chance to bail!
	if Msg::plain("\x1b[1;38;5;199m…Hooray?\x1b[0m").prompt_with_default(true) {
		eprintln!("\n");
		Ok(())
	}
	else {
		eprintln!();
		Err(RipRipError::Killed)
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

#[inline]
/// # Bool to Yes/No Cow.
const fn yesno(v: bool) -> Cow<'static, str> {
	if v { Cow::Borrowed("Yes") }
	else { Cow::Borrowed("No") }
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
        --cutoff <NUM>
                      Consider allegedly-good samples \"likely\" once the same
                      value has been read at least <NUM> times, and twice as
                      often as any competing values. Sectors containing only
                      likely/confirmed samples are skipped during subsequent
                      passes, so the lower the cutoff, the faster they'll go.
                      Higher values are recommended when the data seems fishy.
                      [default: 2; range: 1..=32]
        --raw         Save ripped tracks in raw PCM format (instead of WAV).
    -r, --refine <NUM>
                      Automatically execute up to <NUM> additional rip passes
                      for each track while any samples remain unread or
                      unconfirmed. [default: 0; max: 32]
    -t, --track <NUM(s),RNG>
                      Rip one or more specific tracks (rather than the whole
                      disc). Multiple tracks can be separated by commas (2,3),
                      specified as an inclusive range (2-3), and/or given their
                      own -t/--track (-t 2 -t 3). Include track 0 to rip the
                      HTOA (if any). [default: the whole disc]

WHEN ALL ELSE FAILS:
        --backwards   Rip sectors in reverse order. (Data will still be saved
                      in the *correct* order. Haha.)
        --no-resume   Ignore any previous rip states; start over from scratch.
        --strict      Treat C2 errors as an all-or-nothing proposition for the
                      sector as a whole rather than judging each individual
                      sample on its own. This is most effective when set for
                      all rip passes (rather than being turned on after several
                      runs have already completed).

DRIVE SETTINGS:
    These options are auto-detected and do not usually need to be explicitly
    provided.

    -d, --dev <PATH>  The device path for the optical drive containing the CD
                      of interest, like /dev/cdrom.
    -o, --offset <SAMPLES>
                      The AccurateRip, et al, sample read offset to apply to
                      data retrieved from the drive. [range: ±5880]

UNUSUAL SETTINGS:
        --confidence <NUM>
                      Consider a track accurately ripped — i.e. stop working on
                      it — AccurateRip and/or CUETools matches are found with a
                      confidence of at least <NUM>. [default: 3; range: 3..=10]
        --no-c2       Disable/ignore C2 error pointer information when ripping,
                      e.g. for drives that do not support the feature. (This
                      flag is otherwise not recommended.)
        --no-cache-bust
                      Do not attempt to reset the optical drive cache between
                      each rip pass.

MISCELLANEOUS:
    -h, --help        Print help information and exit.
    -V, --version     Print version information and exit.
        --no-rip      Print the basic drive and disc information to STDERR and
                      exit (without ripping anything).
        --no-summary  Skip the drive and disc summary and jump straight to
                      ripping.

EARLY EXIT:
    If you don't have time to let a rip finish naturally, press "#, "\x1b[38;5;208mCTRL\x1b[0m+\x1b[38;5;208mC\x1b[0m to stop
    it early. Your progress will still be saved, there just won't be as much of
    it. Haha.
"
	));
}
