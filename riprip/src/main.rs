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

#![expect(clippy::redundant_pub_crate, reason = "Unresolvable.")]



mod cli;

use dactyl::NiceU16;
use fyi_msg::{
	Msg,
	Progless,
};
use oxford_join::JoinFmt;
use riprip_core::{
	Disc,
	KillSwitch,
	RipRipError,
	RipOptions,
};
use std::{
	borrow::Cow,
	fmt,
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
	match main__() {
		Ok(()) => {},
		Err(e @ (RipRipError::PrintHelp | RipRipError::PrintVersion)) => {
			println!("{e}");
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
fn main__() -> Result<(), RipRipError> {
	let (
		opts,
		disc,
		drivevendormodel,
		no_rip,
		no_summary,
		status,
	) = cli::parse()?;

	// Quiet?
	if ! no_summary {
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
	if no_rip { return Ok(()); }

	// Set up progress and killswitch in case they're needed.
	let killed = KillSwitch::from(Progless::sigint_keepalive());
	let progress = Progless::default();

	// Just checking the status?
	if status { return disc.status(&opts, &progress, killed); }

	// Parse the options.
	rip_summary(&disc, &opts)?;

	// Log header.
	if opts.verbose() { log_header(&disc, &opts); }

	// Rip and rip and rip!
	disc.rip(&opts, &progress, killed)?;

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
	#[derive(Copy, Clone)]
	/// # Track Number(s).
	enum SummaryTrack {
		/// # One Track.
		One(u8),

		/// # Track Range.
		Rng(u8, u8),
	}

	impl fmt::Display for SummaryTrack {
		#[inline]
		fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
			match *self {
				Self::One(n) => write!(f, "{n}"),
				Self::Rng(a, b) => write!(f, "{a}\x1b[0;2m..=\x1b[0;1m{b}"),
			}
		}
	}

	let mut set = opts.tracks_rng()
		.map(|rng| {
			let (a, b) = rng.into_inner();
			if a == b { SummaryTrack::One(a) }
			else { SummaryTrack::Rng(a, b) }
		})
		.collect::<Vec<_>>();

	match set.len() {
		1 => set.remove(0).to_string(),
		2 => format!(
			"{}\x1b[0;2m and \x1b[0;1m{}",
			set[0],
			set[1],
		),
		_ => set.pop().map_or_else(String::new, |last| format!(
			"{}\x1b[0;2m, and \x1b[0;1m{last}",
			JoinFmt::new(set.into_iter(), "\x1b[0;2m, \x1b[0;1m"),
		)),
	}
}
