/*!
# Rip Rip Hooray: Library
*/

#![deny(
	clippy::allow_attributes_without_reason,
	clippy::correctness,
	unreachable_pub,
	unsafe_code,
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

#![expect(clippy::doc_markdown, reason = "`RipRip` makes this annoying.")]
#![expect(clippy::redundant_pub_crate, reason = "Unresolvable.")]

mod abort;
mod barcode;
mod cache;
mod cdio;
mod cdtext;
mod chk;
mod disc;
mod drive;
mod error;
mod rip;

pub use abort::KillSwitch;
pub use barcode::Barcode;
pub(crate) use cache::{
	cache_path,
	cache_prefix,
	CacheWriter,
	state_path,
	track_path,
};
pub(crate) use cdio::LibcdioInstance;
pub use cdtext::CDTextKind;
pub(crate) use chk::{
	chk_accuraterip,
	chk_ctdb,
};
pub use disc::Disc;
pub use drive::{
	DriveVendorModel,
	ReadOffset,
};
pub use error::RipRipError;
pub(crate) use rip::{
	buf::RipBuffer,
	data::RipState,
	sample::RipSample,
	Ripper,
};
pub use rip::opts::RipOptions;
use std::{
	collections::BTreeMap,
	path::PathBuf,
};



/// # 16-bit Stereo Sample (raw PCM bytes).
type Sample = [u8; 4];

/// # Ripper::Finish Return Type.
type SavedRips = BTreeMap<u8, (PathBuf, Option<(u8, u8)>, Option<u16>)>;



// Cache
// ---------------

/// # Cache Base.
///
/// The cache root is thus `CWD/CACHE_BASE`.
pub const CACHE_BASE: &str = "_riprip";

/// # Cache Scratch.
///
/// The scratch folder for non-track data, e.g. `CWD/CACHE_BASE/CACHE_SCRATCH`.
const CACHE_SCRATCH: &str = "scratch";



// Color
// ---------------

/// # Color: Bad.
const COLOR_BAD: &str = "91";

/// # Color: Maybe.
const COLOR_MAYBE: &str = "38;5;208";

/// # Color: Likely.
const COLOR_LIKELY: &str = "93";

/// # Color: Confirmed.
const COLOR_CONFIRMED: &str = "92";



// Conversion
// ---------------

/// # Bytes Per Sample.
const BYTES_PER_SAMPLE: u16 = 4;

/// # Bytes Per Sector.
///
/// This is the number of bytes per sector of _audio_ data. Block sizes may
/// contain additional information.
const BYTES_PER_SECTOR: u16 = SAMPLES_PER_SECTOR * BYTES_PER_SAMPLE;

/// # Samples per sector.
const SAMPLES_PER_SECTOR: u16 = 588;

/// # Sample Overread (Padding).
///
/// To help account for variable read offsets and CTDB matching, each track rip
/// will overread up to ten sectors on either end.
const SAMPLE_OVERREAD: u16 = SAMPLES_PER_SECTOR * SECTOR_OVERREAD;

/// # Sector Overread (Padding).
const SECTOR_OVERREAD: u16 = 10;



// Block Sizes
// ---------------

/// # Size of C2 block.
///
/// Note: some drives support a 296-byte variation with an extra block bit, but
/// such drives should also support the 294-bit version, and that extra bit is
/// redundant.
const CD_C2_SIZE: u16 = 294;

/// # Size of (Formatted) Subchannel Block.
const CD_SUBCHANNEL_SIZE: u16 = 16;

/// # Size of data block.
///
/// Data as in "audio data".
const CD_DATA_SIZE: u16 = BYTES_PER_SECTOR;

/// # Combined size of data/c2.
const CD_DATA_C2_SIZE: u16 = CD_DATA_SIZE + CD_C2_SIZE;

/// # Combined size of data/subchannel.
const CD_DATA_SUBCHANNEL_SIZE: u16 = CD_DATA_SIZE + CD_SUBCHANNEL_SIZE;



// Misc
// ---------------

/// # Number of lead-in sectors.
///
/// All discs have a 2-second region at the start before any data. Different
/// contexts include or exclude this amount, so it's good to keep it handy.
const CD_LEADIN: u16 = 150;

/// # Lead-out Label.
///
/// This is used solely for the table of contents printout; e.g. 01 02 03 AA.
const CD_LEADOUT_LABEL: &str = "AA";

/// # Null sample.
///
/// Audio CD silence is typically literally nothing.
const NULL_SAMPLE: Sample = [0, 0, 0, 0];
