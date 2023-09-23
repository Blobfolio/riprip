/*!
# Rip Rip Hooray: Build

This downloads and parses the AccurateRip drive offset list into a constant
array that can be easily searched at runtime.
*/

use std::{
	collections::BTreeMap,
	fs::{
		File,
		Metadata,
	},
	io::{
		Read,
		Write,
	},
	path::{
		Path,
		PathBuf,
	},
};



/// # Glumped Vendor/Model.
///
/// This mirrors the DriveVendorModel type in the living program.
type VendorModel = [u8; 24];

/// # The remote URL of the data.
const DATA_URL: &str = "http://www.accuraterip.com/accuraterip/DriveOffsets.bin";

/// # Min Offset.
const MIN_OFFSET: i16 = -5880;

/// # Max Offset.
const MAX_OFFSET: i16 = 5880;

/// # Max Vendor Length.
const MAX_VENDOR_LEN: usize = 8;

/// # Max Model Length.
const MAX_MODEL_LEN: usize = 16;



/// # Main.
fn main() {
	println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");
	println!("cargo:rerun-if-changed=skel");

	let raw = fetch_offsets();
	let offsets = parse_offsets(&raw);
	let caches = parse_caches(&offsets);

	// Save the offsets.
	let dst = out_path("drives.rs");
	let data = [nice_caches(caches), nice_offsets(offsets)].concat();
	File::create(dst)
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
	let res = ureq::get(DATA_URL)
		.set("user-agent", "Mozilla/5.0")
		.call()
		.expect("Unable to download AccurateRip drive offsets.");

	let mut out: Vec<u8> = Vec::new();
	res.into_reader().read_to_end(&mut out)
		.expect("Unable to read the AccurateRip drive offset server response.");

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

	println!("cargo:warning=Found {} drive caches in the database.", nice.len());

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
	let mut min = i16::MAX;
	let mut max = i16::MIN;
	let nice = parsed.into_iter()
		.map(|(vendormodel, offset)| {
			if offset < min { min = offset; }
			if offset > max { max = offset; }
			format!("(DriveVendorModel({vendormodel:?}), ReadOffset({offset})),")
		})
		.collect::<Vec<String>>();

	// Announce the count so the builder can see what's going on under the
	// hood. There should be a few thousand.
	println!("cargo:warning=Found {} drive offsets in the database.", nice.len());
	println!("cargo:warning=Min offset: {min}.");
	println!("cargo:warning=Max offset:  {max}.");

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
	let dir = std::env::var("OUT_DIR").expect("Missing OUT_DIR.");
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
	let mut parsed: BTreeMap<VendorModel, i16> = BTreeMap::new();

	// Run through each entry.
	for chunk in raw.chunks_exact(69) {
		// Parsing numbers is so nice!
		let offset = i16::from_le_bytes([chunk[0], chunk[1]]);

		// Ignore entries with an offset of zero (our default) as well as
		// anything beyond our supported range, although at present no entries
		// come close to that.
		if offset == 0 || ! (MIN_OFFSET..=MAX_OFFSET).contains(&offset) { continue; }

		// The drive ID may be null-padded on the end. Let's trim those away.
		let mut drive_id = &chunk[2..34];
		while let [ rest @ .., 0 ] = drive_id { drive_id = rest; }

		// Make sure it is valid UTF-8.
		let Ok(drive_id) = std::str::from_utf8(drive_id) else { continue; };

		// Both the vendor and model have fixed lengths on the hardware side;
		// we can store them together to make the search more efficient. This
		// structure matches `DriveVendorModel` in our source.
		let mut vendormodel = VendorModel::default();

		// AccurateRip doesn't take advantage of the inherent field sizes. It
		// concatenates the two with " - " instead, or "- " in cases where the
		// vendor part is absent.

		// Let's check for no-vendor first.
		if let Some(mut model) = drive_id.strip_prefix("- ") {
			model = model.trim();

			// Model is required and must fit within its maximum length.
			if (1..=MAX_MODEL_LEN).contains(&model.len()) {
				// Pretty sure these have to be ASCII.
				if ! model.is_ascii() {
					println!("cargo:warning=Non-ASCII model {model}.");
					continue;
				}

				for (b, v) in vendormodel.iter_mut().skip(MAX_VENDOR_LEN).zip(model.bytes()) {
					*b = v.to_ascii_uppercase();
				}
				if let Some(offset1) = parsed.insert(vendormodel, offset) {
					if offset1 != offset {
						println!("cargo:warning=Multiple offsets: [no vendor] / {model} ({offset1}, {offset}).");
					}
				}
			}
			else {
				println!("cargo:warning=Invalid: [no vendor] / {model}.");
			}
		}
		// Otherwise it will look like "VENDOR - MODEL".
		else {
			let mut split = drive_id.splitn(2, " - ");
			let Some(mut vendor) = split.next() else { continue; };
			let Some(mut model) = split.next() else { continue; };
			vendor = vendor.trim();
			model = model.trim();

			// Both are required and must fit within their maximum lengths.
			if (1..=MAX_VENDOR_LEN).contains(&vendor.len()) && (1..=MAX_MODEL_LEN).contains(&model.len()) {
				// Pretty sure these have to be ASCII.
				if ! vendor.is_ascii() || ! model.is_ascii() {
					println!("cargo:warning=Non-ASCII vendor/model {vendor} / {model}.");
					continue;
				}

				for (b, v) in vendormodel.iter_mut().zip(vendor.bytes()) {
					*b = v.to_ascii_uppercase();
				}
				for (b, v) in vendormodel.iter_mut().skip(MAX_VENDOR_LEN).zip(model.bytes()) {
					*b = v.to_ascii_uppercase();
				}

				// Add it!
				if let Some(offset1) = parsed.insert(vendormodel, offset) {
					if offset1 != offset {
						println!("cargo:warning=Multiple offsets: {vendor} / {model} ({offset1}, {offset}).");
					}
				}
			}
			else {
				println!("cargo:warning=Invalid: {vendor} / {model}.");
			}
		}
	}

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
