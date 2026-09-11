/*!
# Rip Rip Hooray: Build

This downloads and parses the AccurateRip drive offset list into a constant
array that can be easily searched at runtime.
*/

use cdtoc::AccurateRip;
use dactyl::{
	NiceSeparator,
	NiceU16,
};
use oxford_join::JoinFmt;
use std::{
	collections::BTreeMap,
	env,
	fmt,
	fs::File,
	io::Write,
	path::PathBuf,
};



/// # Glumped Vendor/Model.
///
/// This mirrors the DriveVendorModel type in the living program.
type VendorModel = [u8; 24];



/// # Main.
fn main() {
	println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");
	println!("cargo:rerun-if-changed=skel/drive-cache.txt");
	println!("cargo:rerun-if-changed=skel/drive-offsets.bin");

	let offsets = parse_offsets();
	let caches = parse_caches(&offsets);

	// Announce the totals for reference.
	if env::var("SHOW_TOTALS").is_ok() {
		let len = offsets.len().to_string().len();
		println!("cargo:warning=Read Offsets: {}", offsets.len());
		println!("cargo:warning=Cache Sizes:  {:>len$}", caches.len());
	}

	// Save it!
	let data = [nice_caches(caches), nice_offsets(offsets)].concat();
	File::create(out_path("drives.rs"))
		.and_then(|mut f| f.write_all(data.as_bytes()).and_then(|_| f.flush()))
		.expect("Unable to save drive data.");
}



/// # Nice Drive Caches.
///
/// Reformat the cache sizes as Rust code that can be included directly in a
/// library script.
///
/// The generated code takes the form of a static array, allowing for
/// reasonably fast and straightforward binary search at runtime.
fn nice_caches(parsed: BTreeMap<VendorModel, u16>) -> String {
	/// # Vendor/Model and Size.
	struct VendorModelSize(VendorModel, u16);

	impl fmt::Display for VendorModelSize {
		#[inline]
		fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
			write!(
				f,
				"(DriveVendorModel({:?}), {}_u16)",
				self.0,
				NiceU16::with_separator(self.1, NiceSeparator::Underscore),
			)
		}
	}

	// Reformat the data into "code" for the array we're about to generate.
	format!(
		r#"
/// # Drive Cache Sizes.
const DRIVE_CACHES: [(DriveVendorModel, u16); {}] = [
	{}
];
"#,
		parsed.len(),
		JoinFmt::new(parsed.into_iter().map(|(x, y)| VendorModelSize(x, y)), ", "),
	)
}

/// # Nice Drive Offsets.
///
/// Reformat the offsets as Rust code that can be included directly in a
/// library script.
///
/// The generated code takes the form of a static array, allowing for
/// reasonably fast and straightforward binary search at runtime.
fn nice_offsets(parsed: BTreeMap<VendorModel, i16>) -> String {
	/// # Vendor/Model and Size.
	struct VendorModelOffset(VendorModel, i16);

	impl fmt::Display for VendorModelOffset {
		#[inline]
		fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
			write!(
				f,
				"(DriveVendorModel({:?}), ReadOffset({}))",
				self.0,
				self.1,
			)
		}
	}

	// Reformat the data into "code" for the array we're about to generate.
	format!(
		r#"
/// # Drive Offsets.
static DRIVE_OFFSETS: [(DriveVendorModel, ReadOffset); {}] = [
	{},
];
"#,
		parsed.len(),
		JoinFmt::new(parsed.into_iter().map(|(x, y)| VendorModelOffset(x, y)), ", "),
	)
}

/// # Out path.
///
/// This generates a (file/dir) path relative to `OUT_DIR`.
fn out_path(name: &str) -> PathBuf {
	let dir = env::var("OUT_DIR").expect("Missing OUT_DIR.");
	let mut out = std::fs::canonicalize(dir).expect("Missing OUT_DIR.");
	out.push(name);
	out
}

/// # Parse Drive Caches.
///
/// This essentially transforms our hard-coded `CACHES` array into a `BTreeMap`,
/// but checks to make sure the values are present in the offset list first,
/// just to rule out typos or weird data.
fn parse_caches(offsets: &BTreeMap<VendorModel, i16>) -> BTreeMap<VendorModel, u16> {
	let mut parsed: BTreeMap<VendorModel, u16> = BTreeMap::new();

	let raw = std::fs::read_to_string("skel/drive-cache.txt")
		.expect("Unable to open skel/drive-cache.txt");
	for line in raw.lines() {
		if line.starts_with('#') { continue; }
		let Some((vm, kb)) = parse_cache_line(line) else {
			println!("cargo:warning=Invalid cache line: {line}.");
			continue;
		};
		if offsets.contains_key(&vm) { parsed.insert(vm, kb); }
		else {
			println!("cargo:warning=Unknown cache vendor/model: {line}.");
		}
	}

	parsed
}

/// # Parse a Single Cache Entry.
///
/// Tease out the vendor/model and cache size from the string and return them.
fn parse_cache_line(line: &str) -> Option<(VendorModel, u16)> {
	// To make the data file easier to read, null bytes are replaced with
	// ellipses; first things first we need to convert those back. The result
	// should be a line of ASCII, at least 24 (vm) + 1 (space) + 1 (size) long.
	let line = line.replace('…', "\0");
	if ! line.is_ascii() || line.len() < 26 { return None; }

	// Parse the two halves.
	let (vm, kb) = line.split_at(24);
	let vm: VendorModel = vm.as_bytes().try_into().ok()?;
	let kb: u16 = kb.trim().parse().ok()?;

	// Cache can't be zero.
	if kb == 0 { None }
	//Otherwise return what we've got!
	else { Some((vm, kb)) }
}

/// # Parse Drive Offsets.
///
/// The raw bin data is stored in fixed-length chunks of 69 bytes that break
/// down as follows:
/// * 02 byte i16 offset
/// * 32 byte glumped vendor/model string
/// * 01 byte string terminator
/// * 01 byte u8 submission count
/// * 33 bytes (unused by the look of it)
///
/// We only care about the first two parts.
fn parse_offsets() -> BTreeMap<VendorModel, i16> {
	let raw = std::fs::read("skel/drive-offsets.bin")
		.expect("Unable to open skel/drive-offsets.bin");

	// CDTOC does most of the work for us, but we can ignore 0-offset entries,
	// and will uppercase the vendor/model pairs for case-insensitive
	// searching.
	let parsed: BTreeMap<VendorModel, i16> = AccurateRip::parse_drive_offsets(&raw)
		.expect("Unable to parse drive offsets.")
		.into_iter()
		.filter_map(|((v, m), o)|
			if o == 0 { None }
			else {
				// Reformat the vendor/model pairs into our array.
				let mut vm = VendorModel::default();
				if ! v.is_empty() {
					for (old, new) in vm.iter_mut().zip(v.bytes()) {
						*old = new.to_ascii_uppercase();
					}
				}
				for (old, new) in vm.iter_mut().skip(8).zip(m.bytes()) {
					*old = new.to_ascii_uppercase();
				}

				// And return!
				Some((vm, o))
			}
		)
		.collect();

	// Make sure we parsed something.
	if parsed.is_empty() { panic!("No drive offsets could be parsed."); }

	// Done!
	parsed
}
