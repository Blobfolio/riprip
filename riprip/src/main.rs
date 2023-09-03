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

	// Clean cache first.
	if args.switch(b"--clean") {
		riprip_core::cache_clean()?;
		Msg::info("Cleaned the cache!").eprint();
	}

	// Connect to the device and summarize the disc.
	let dev = args.option2_os(b"-d", b"--dev");
	let disc = Disc::new(dev)?;
	let drivevendormodel = disc.drive_vendor_model();
	if let Some(vm) = drivevendormodel {
		let vm = vm.to_string();
		eprintln!("\x1b[1;36m{vm}\x1b[0m");
		eprintln!("\x1b[2;36m{}\x1b[0m\n", "-".repeat(vm.len()));
	}
	eprintln!("{disc}");

	// Go ahead and leave if there's no ripping to do.
	if args.switch(b"--no-rip") { return Ok(()); }

	// Set up the ripper!
	let opts = parse_rip_options(&args, drivevendormodel)?;
	let progress = Progless::default();
	let killed = KillSwitch::default();
	sigint(killed.inner(), Some(progress.clone()));

	// Summarize.
	rip_summary(&opts)?;

	// Rip and rip and rip!
	disc.rip(&opts, &progress, &killed)?;

	eprintln!();
	if killed.killed() { Err(RipRipError::Killed) }
	else {
		Msg::success("That's all folks!").eprint();
		Ok(())
	}
}

/// # Parse Rip Options.
fn parse_rip_options(args: &Argue, drive: Option<DriveVendorModel>) -> Result<RipOptions, RipRipError> {
	let mut opts = RipOptions::default()
		.with_c2(! args.switch(b"--no-c2"))
		.with_raw(args.switch(b"--raw"))
		.with_reconfirm(args.switch(b"--reconfirm"));

	// Detect offset?
	if let Some(v) = drive.and_then(|vm| vm.detect_offset()) {
		opts = opts.with_offset(v);
	}

	if let Some(v) = args.option2(b"-o", b"--offset") {
		let offset = ReadOffset::try_from(v)?;
		opts = opts.with_offset(offset);
	}

	if let Some(v) = args.option(b"--paranoia") {
		let paranoia = u8::btou(v).ok_or(RipRipError::Paranoia)?;
		opts = opts.with_paranoia(paranoia);
	}

	if let Some(v) = args.option(b"--refine") {
		let refine = u8::btou(v).ok_or(RipRipError::Refine)?;
		opts = opts.with_refine(refine);
	}

	// Parsing the tracks is slightly more involved. Haha.
	let mut tracks = Vec::new();
	for v in args.option2_values(b"-t", b"--track", Some(b',')) {
		let v = v.trim();
		if v.is_empty() { continue; }

		if let Some(pos) = v.iter().position(|b| b'-'.eq(b)) {
			let a = v[..pos].trim();
			let b = v[pos + 1..].trim();
			if a.is_empty() || b.is_empty() {
				return Err(RipRipError::RipTracks);
			}

			let a = u8::btou(a).ok_or(RipRipError::RipTracks)?;
			let b = u8::btou(b).ok_or(RipRipError::RipTracks)?;
			if a <= b { tracks.extend(a..=b); }
			else { tracks.extend(b..=a); }
		}
		else {
			let v = u8::btou(v).ok_or(RipRipError::RipTracks)?;
			tracks.push(v);
		}
	}
	if ! tracks.is_empty() {
		opts = opts.with_tracks(tracks);
	}

	// Conflict checks.
	if opts.reconfirm() && opts.paranoia() < 2 {
		return Err(RipRipError::ReconfirmParanoia);
	}

	// Done!
	Ok(opts)
}

/// # Rip Summary.
fn rip_summary(opts: &RipOptions) -> Result<(), RipRipError> {
	use oxford_join::OxfordJoin;

	let tracks = opts.tracks()
		.iter()
		.map(|&t| NiceU8::from(t))
		.collect::<Vec<NiceU8>>();
	let nice_tracks =
		if tracks.is_empty() { Cow::Borrowed("EVERYTHING") }
		else { tracks.oxford_and() };
	let nice_c2 = Cow::Borrowed(if opts.c2() { "Yes" } else { "No" });
	let nice_format = Cow::Borrowed(if opts.raw() { "Raw PCM" } else { "WAV" });
	let nice_offset = Cow::Owned(format!("{}", opts.offset().samples()));
	let nice_paranoia = NiceU8::from(opts.paranoia());
	let nice_passes = NiceU8::from(opts.passes());
	let nice_reconfirm = Cow::Borrowed(if opts.reconfirm() { "Yes" } else { "No" });

	let set = [
		("Tracks:", nice_tracks, true),
		("C2:", nice_c2, opts.c2()),
		("Format:", nice_format, true),
		("Offset:", nice_offset, 0 != opts.offset().samples_abs()),
		("Paranoia:", Cow::Borrowed(nice_paranoia.as_str()), 1 < opts.paranoia()),
		("Passes:", Cow::Borrowed(nice_passes.as_str()), true),
		("Reconfirm:", nice_reconfirm, opts.reconfirm()),
	];

	eprintln!("\x1b[1;38;5;199mRip Rip…\x1b[0m");
	for (k, v, enabled) in set {
		if enabled {
			eprintln!("  {k:10} \x1b[1m{v}\x1b[0m");
		}
		else {
			eprintln!("  \x1b[2m{k:10} {v}\x1b[0m");
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

#[cold]
/// # Print Help.
fn helper() {
	println!(concat!(
		r"
 ╚⊙ ⊙╝
╚═(███)═╝
 ╚═(███)═╝
  ╚═(███)═╝
   ╚═(███)═╝
   ╚═(███)═╝
  ╚═(███)═╝
 ╚═(███)═╝    ", "\x1b[38;5;199mRip Rip Hooray!\x1b[0;38;5;69m v", env!("CARGO_PKG_VERSION"), "\x1b[0m", r"
╚═(███)═╝     Accurate, incremental
╚═(███)═╝     raw audio CD ripping.
 ╚═(███)═╝
  ╚═(███)═╝
   ╚═(███)═╝
     ╚═(█)═╝

USAGE:
    riprip [FLAGS] [OPTIONS]

FLAGS:
        --clean       Clear any previous riprip/state files from the current
                      working directory before doing anything else.
    -h, --help        Print help information and exit.
        --no-c2       Disable/ignore C2 error pointer information when ripping,
                      e.g. for drives that do not support the feature. (This
                      flag is otherwise not recommended.)
        --no-rip      Just print the basic disc information to STDERR and exit.
        --raw         Save ripped tracks in raw PCM format (instead of WAV).
        --reconfirm   Reset the status of all previously-accepted samples to
                      require reconfirmation. This has no effect when the
                      paranoia level is less than 2.
    -V, --version     Print version information and exit.

OPTIONS:
        --paranoia <NUM>
                      When C2 or read errors are reported for any samples in a
                      given block, treat the rest of its samples — the ones
                      that were allegedly good — as suspicious until they have
                      been confirmed <NUM> times. Similarly, if a sample moves
                      from bad to good, require <NUM> confirmations before
                      believing it. [default: 3; range: 1..=32]
        --refine <NUM>
                      Execute up to <NUM> additional rip passes for each track
                      while any samples remain unread/unconfirmed.
                      [default: 0; max: 15]
    -t, --track <NUM(s),RNG>
                      Rip one or more specific tracks (rather than the whole
                      disc). Multiple tracks can be separated by commas (2,3),
                      specified as an inclusive range (2-3), and/or given their
                      own -t/--track (-t 2 -t 3). [default: the whole disc]

DRIVE OPTIONS:

    These options are auto-detected and do not usually need to be explicitly
    provided.

    -d, --dev <PATH>  The device path for the optical drive containing the CD
                      of interest, like /dev/cdrom.
    -o, --offset <SAMPLES>
                      The AccurateRip, et al, sample read offset to apply to
                      data retrieved from the drive. [range: ±5880]
"
	));
}
