/*!
# Rip Rip Hooray: Build

This downloads and parses the AccurateRip drive offset list into a constant
array that can be easily searched at runtime.
*/

use cdtoc::AccurateRip;
use std::{
	collections::BTreeMap,
	env,
	fs::{
		File,
		Metadata,
	},
	io::Write,
	path::{
		Path,
		PathBuf,
	},
};



/// # Glumped Vendor/Model.
///
/// This mirrors the DriveVendorModel type in the living program.
type VendorModel = [u8; 24];



/// # Main.
fn main() {
	println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");
	println!("cargo:rerun-if-changed=skel");

	let raw = fetch_offsets();
	let offsets = parse_offsets(&raw);
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



/// # Download/Cache Raw Data.
///
/// This will try to pull the data from the build cache if it exists, otherwise
/// it will download it fresh (and save it to the build cache for next time).
fn fetch_offsets() -> Vec<u8> {
	// Pull from cache?
	let cache = out_path("DriveOffsets.bin");
	if let Some(x) = try_cache(&cache) { return x; }

	// Download it fresh.
	let res = minreq::get(AccurateRip::DRIVE_OFFSET_URL)
		.with_header("user-agent", "Mozilla/5.0")
		.send()
		.expect("Unable to download AccurateRip drive offsets.");

	// Only accept happy response codes with sized bodies.
	if ! (200..=399).contains(&res.status_code) {
		panic!("AccurateRip returned {}.", res.status_code);
	}

	let out = res.into_bytes();
	if out.is_empty() {
		panic!("The AccurateRip drive offset server response was empty.");
	}

	// Try to cache for next time.
	let _res = File::create(cache)
		.and_then(|mut f| f.write_all(&out).and_then(|_| f.flush()));

	out
}

/// # Nice Drive Caches.
///
/// Reformat the cache sizes as Rust code that can be included directly in a
/// library script.
///
/// The generated code takes the form of a static array, allowing for
/// reasonably fast and straightforward binary search at runtime.
fn nice_caches(parsed: BTreeMap<VendorModel, u16>) -> String {
	// Reformat the data into "code" for the array we're about to generate.
	let nice = parsed.into_iter()
		.map(|(vendormodel, size)| format!(
			"(DriveVendorModel({vendormodel:?}), {size}_u16),"
		))
		.collect::<Vec<String>>();

	format!(
		r#"
/// # Drive Cache Sizes.
const DRIVE_CACHES: [(DriveVendorModel, u16); {}] = [
	{}
];
"#,
		nice.len(),
		nice.join(" "),
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
	use std::fmt::Write;

	// Reformat the data into "code" for the array we're about to generate.
	let nice = parsed.into_iter()
		.map(|(vendormodel, offset)|
			format!("(DriveVendorModel({vendormodel:?}), ReadOffset({offset})),")
		)
		.collect::<Vec<String>>();

	// Start the array.
	let mut out = format!(
		r#"
/// # Drive Offsets.
const DRIVE_OFFSETS: [(DriveVendorModel, ReadOffset); {}] = ["#,
		nice.len(),
	);

	// Split up the data so we don't end up with one REALLY LONG line.
	for chunk in nice.chunks(256) {
		write!(&mut out, "\n\t{}", chunk.join(" ")).expect("Failed to write string.");
	}

	// Close out the array.
	out.push_str("\n];\n");
	out
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
fn parse_offsets(raw: &[u8]) -> BTreeMap<VendorModel, i16> {
	// CDTOC does most of the work for us, but we can ignore 0-offset entries,
	// and will uppercase the vendor/model pairs for case-insensitive
	// searching.
	let parsed: BTreeMap<VendorModel, i16> = AccurateRip::parse_drive_offsets(raw)
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

/// # Try Cache.
///
/// Return a previously-cached copy of the raw data (from `target`), unless it
/// doesn't exist or was generated more than a day ago.
fn try_cache(path: &Path) -> Option<Vec<u8>> {
	std::fs::metadata(path)
		.ok()
		.filter(Metadata::is_file)
		.and_then(|meta| meta.modified().ok())
		.and_then(|time| time.elapsed().ok().filter(|secs| secs.as_secs() < 86400))
		.and_then(|_| std::fs::read(path).ok())
}
