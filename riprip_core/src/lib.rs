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
	clippy::implicit_clone,
	clippy::lossy_float_literal,
	clippy::missing_assert_message,
	clippy::missing_docs_in_private_items,
	clippy::needless_raw_strings,
	clippy::panic_in_result_fn,
	clippy::pub_without_shorthand,
	clippy::rest_pat_in_fully_bound_structs,
	clippy::semicolon_inside_block,
	clippy::str_to_string,
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

#[cfg(not(target_pointer_width = "64"))]
compile_error!("Rip Rip requires a 64-bit CPU architecture.");



mod abort;
mod barcode;
mod cache;
pub mod cdtext;
mod chk;
mod disc;
mod drive;
mod drivers;
mod error;
mod isrc;
mod loglog;
pub mod macros;
mod rip;
mod subq;
mod track;



pub use abort::KillSwitch;
pub use barcode::Barcode;
pub use cdtoc;
pub use disc::Disc;
pub use drive::{
	DriveVendorModel,
	ReadOffset,
};
pub use error::RipRipError;
pub use isrc::{
	Isrc,
	IsrcMap,
};
pub use loglog::{
	LogLog,
	LogLevel,
};
pub use rip::opts::RipOptions;

use cache::{
	cache_path,
	cache_prefix,
	CacheWriter,
	state_path,
	track_path,
};
use chk::{
	chk_accuraterip,
	chk_ctdb,
};
use drivers::{
	CddaDriver,
	CddaDriverExt,
	CddaDriverNewExt,
};
use rip::{
	buf::RipBuffer,
	data::RipState,
	manifest::RipManifest,
	sample::RipSample,
	Ripper,
};
use std::{
	collections::BTreeMap,
	path::PathBuf,
};
use subq::SubQ;
use track::TrackRange;



/// # Helper: Count.
///
/// Thanks Little Book of Rust Macros!
macro_rules! count {
	() => ( 0 );
	($odd:tt) => ( 1 );
	($odd:tt $( $a:tt $b:tt )+) => ( ($crate::count!($($a)+) * 2) + 1 );
	($( $a:tt $b:tt )+) =>         (  $crate::count!($($a)+) * 2      );
}
use count;



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

/// # Frames Per Second.
const FRAMES_PER_SECOND: u8 = 75;

/// # Lead-out Track Number.
pub const CD_LEADOUT: u8 = 0xAA;

/// # Null sample.
///
/// Audio CD silence is typically literally nothing.
const NULL_SAMPLE: Sample = [0, 0, 0, 0];

/// # Fixed CRC16 Lookup Table.
const CRC: [u16; 256] = [
	0x0000, 0x1021, 0x2042, 0x3063, 0x4084, 0x50A5, 0x60C6, 0x70E7, 0x8108, 0x9129, 0xA14A, 0xB16B, 0xC18C, 0xD1AD, 0xE1CE, 0xF1EF,
	0x1231, 0x0210, 0x3273, 0x2252, 0x52B5, 0x4294, 0x72F7, 0x62D6, 0x9339, 0x8318, 0xB37B, 0xA35A, 0xD3BD, 0xC39C, 0xF3FF, 0xE3DE,
	0x2462, 0x3443, 0x0420, 0x1401, 0x64E6, 0x74C7, 0x44A4, 0x5485, 0xA56A, 0xB54B, 0x8528, 0x9509, 0xE5EE, 0xF5CF, 0xC5AC, 0xD58D,
	0x3653, 0x2672, 0x1611, 0x0630, 0x76D7, 0x66F6, 0x5695, 0x46B4, 0xB75B, 0xA77A, 0x9719, 0x8738, 0xF7DF, 0xE7FE, 0xD79D, 0xC7BC,
	0x48C4, 0x58E5, 0x6886, 0x78A7, 0x0840, 0x1861, 0x2802, 0x3823, 0xC9CC, 0xD9ED, 0xE98E, 0xF9AF, 0x8948, 0x9969, 0xA90A, 0xB92B,
	0x5AF5, 0x4AD4, 0x7AB7, 0x6A96, 0x1A71, 0x0A50, 0x3A33, 0x2A12, 0xDBFD, 0xCBDC, 0xFBBF, 0xEB9E, 0x9B79, 0x8B58, 0xBB3B, 0xAB1A,
	0x6CA6, 0x7C87, 0x4CE4, 0x5CC5, 0x2C22, 0x3C03, 0x0C60, 0x1C41, 0xEDAE, 0xFD8F, 0xCDEC, 0xDDCD, 0xAD2A, 0xBD0B, 0x8D68, 0x9D49,
	0x7E97, 0x6EB6, 0x5ED5, 0x4EF4, 0x3E13, 0x2E32, 0x1E51, 0x0E70, 0xFF9F, 0xEFBE, 0xDFDD, 0xCFFC, 0xBF1B, 0xAF3A, 0x9F59, 0x8F78,
	0x9188, 0x81A9, 0xB1CA, 0xA1EB, 0xD10C, 0xC12D, 0xF14E, 0xE16F, 0x1080, 0x00A1, 0x30C2, 0x20E3, 0x5004, 0x4025, 0x7046, 0x6067,
	0x83B9, 0x9398, 0xA3FB, 0xB3DA, 0xC33D, 0xD31C, 0xE37F, 0xF35E, 0x02B1, 0x1290, 0x22F3, 0x32D2, 0x4235, 0x5214, 0x6277, 0x7256,
	0xB5EA, 0xA5CB, 0x95A8, 0x8589, 0xF56E, 0xE54F, 0xD52C, 0xC50D, 0x34E2, 0x24C3, 0x14A0, 0x0481, 0x7466, 0x6447, 0x5424, 0x4405,
	0xA7DB, 0xB7FA, 0x8799, 0x97B8, 0xE75F, 0xF77E, 0xC71D, 0xD73C, 0x26D3, 0x36F2, 0x0691, 0x16B0, 0x6657, 0x7676, 0x4615, 0x5634,
	0xD94C, 0xC96D, 0xF90E, 0xE92F, 0x99C8, 0x89E9, 0xB98A, 0xA9AB, 0x5844, 0x4865, 0x7806, 0x6827, 0x18C0, 0x08E1, 0x3882, 0x28A3,
	0xCB7D, 0xDB5C, 0xEB3F, 0xFB1E, 0x8BF9, 0x9BD8, 0xABBB, 0xBB9A, 0x4A75, 0x5A54, 0x6A37, 0x7A16, 0x0AF1, 0x1AD0, 0x2AB3, 0x3A92,
	0xFD2E, 0xED0F, 0xDD6C, 0xCD4D, 0xBDAA, 0xAD8B, 0x9DE8, 0x8DC9, 0x7C26, 0x6C07, 0x5C64, 0x4C45, 0x3CA2, 0x2C83, 0x1CE0, 0x0CC1,
	0xEF1F, 0xFF3E, 0xCF5D, 0xDF7C, 0xAF9B, 0xBFBA, 0x8FD9, 0x9FF8, 0x6E17, 0x7E36, 0x4E55, 0x5E74, 0x2E93, 0x3EB2, 0x0ED1, 0x1EF0,
];
