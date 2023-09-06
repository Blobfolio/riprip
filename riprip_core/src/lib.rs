/*!
# Rip Rip Hooray: Library
*/

#![deny(unsafe_code)]

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

#![allow(
	clippy::doc_markdown,
	clippy::module_name_repetitions,
	clippy::redundant_pub_crate,
)]

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
	cache_read,
	cache_write,
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
	Rip,
	RipSample,
};
pub use rip::opts::RipOptions;



/// # 16-bit Stereo Sample (raw PCM bytes).
type Sample = [u8; 4];

/// # Cache Base.
///
/// The cache root is thus `CWD/CACHE_BASE`.
pub(crate) const CACHE_BASE: &str = "_riprip";

/// # Bytes Per Sample.
pub const BYTES_PER_SAMPLE: u32 = 4;

/// # Bytes Per Sector.
///
/// This is the number of bytes per sector of _audio_ data. Block sizes may
/// contain additional information.
pub const BYTES_PER_SECTOR: u32 = SAMPLES_PER_SECTOR * BYTES_PER_SAMPLE;

/// # Samples per sector.
pub const SAMPLES_PER_SECTOR: u32 = 588;

/// # Size of C2 block.
///
/// Note: some drives support a 296-byte variation with an extra block bit, but
/// such drives should also support the 294-bit version, and that extra bit is
/// redundant.
pub const CD_C2_SIZE: u32 = 294;

/// # Size of data block.
///
/// Data as in "audio data".
pub const CD_DATA_SIZE: u32 = BYTES_PER_SECTOR;

/// # Combined size of data/c2.
pub const CD_DATA_C2_SIZE: u32 = CD_DATA_SIZE + CD_C2_SIZE;

/// # Number of lead-in sectors.
///
/// All discs have a 2-second region at the start before any data. Different
/// contexts include or exclude this amount, so it's good to keep it handy.
pub const CD_LEADIN: u32 = 150;

/// # Lead-out Label.
///
/// This is used solely for the table of contents printout; e.g. 01 02 03 AA.
pub const CD_LEADOUT_LABEL: &str = "AA";

/// # Null sample.
///
/// Audio CD silence is typically literally nothing.
pub const NULL_SAMPLE: Sample = [0, 0, 0, 0];
