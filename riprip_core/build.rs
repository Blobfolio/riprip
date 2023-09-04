/*!
# Rip Rip Hooray: Build

This downloads and parses the AccurateRip drive offset list, exporting it as
an easily-searchable constant array that can be directly embedded in the
program code.
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
	use std::fmt::Write;
	println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

	let raw = fetch();
	let parsed = parse(&raw);

	// Reformat the data into "code" for the array we're about to generate.
	let nice = parsed.into_iter()
		.map(|(vendormodel, offset)| format!(
			"(DriveVendorModel({vendormodel:?}), ReadOffset({offset})),"
		))
		.collect::<Vec<String>>();

	// Start the array.
	let mut out = format!(
		r#"
/// # Drive Offsets.
const DRIVE_OFFSETS: [(DriveVendorModel, ReadOffset); {}] = ["#,
		nice.len(),
	);

	// Split up the data so we don't end up with a single ridiculously-long
	// line.
	for chunk in nice.chunks(256) {
		write!(&mut out, "\n\t{}", chunk.join(" ")).expect("Failed to write string.");
	}

	// Close out the array.
	out.push_str("\n];\n");

	// Save it.
	let dst = out_path("drive-offsets.rs");
	File::create(dst)
		.and_then(|mut f| f.write_all(out.as_bytes()).and_then(|_| f.flush()))
		.expect("Unable to save drive offsets.");
}



/// # Download/Cache Raw Data.
///
/// This will try to pull the data from the build cache if it exists, otherwise
/// it will download it fresh (and save it to the build cache for next time).
fn fetch() -> Vec<u8> {
	// Is it cached?
	let cache = out_path("DriveOffsets.bin");
	if let Some(x) = try_cache(&cache) { return x; }

	fetch_remote(&cache).expect("Unable to download AccurateRip drive offsets.")
}

/// # Fetch Remote.
///
/// Download the data fresh, and save it to the build cache so we can skip this
/// next time around.
fn fetch_remote(cache: &Path) -> Option<Vec<u8>> {
	// Download it.
	let res = ureq::get(DATA_URL)
		.set("user-agent", "Mozilla/5.0")
		.call()
		.ok()?;

	let mut out: Vec<u8> = Vec::new();
	res.into_reader().read_to_end(&mut out).ok()?;

	if out.is_empty() { None }
	else {
		// Try to save for next time.
		let _res = File::create(cache)
			.and_then(|mut f| f.write_all(&out).and_then(|_| f.flush()));

		Some(out)
	}
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

/// # Parse Raw Data.
fn parse(raw: &[u8]) -> BTreeMap<VendorModel, i16> {
	// Each block should have:
	// * 02 byte i16 Offset
	// * 32 byte drive ID string
	// * 01 byte terminator
	// * 01 byte u8 submission count
	// * 33 byte (unknown)
	//
	// For a grand total of 69. We only care about the first two parts.
	let mut parsed: BTreeMap<VendorModel, i16> = BTreeMap::new();
	for chunk in raw.chunks_exact(69) {
		// Numbers are so nice!
		let offset = i16::from_le_bytes([chunk[0], chunk[1]]);

		// Our default offset is zero, so we can safely omit any entries that
		// have that offset. We should also ignore anything with an impossibly
		// large offset, just in case such records are ever added to the
		// database. (Right now there's nothing beyond about ±1500.)
		if offset == 0 || ! (MIN_OFFSET..=MAX_OFFSET).contains(&offset) { continue; }

		// The drive ID is always 32 bytes even if the string is shorter. Let's
		// trim the excess before carving it up.
		let mut drive_id = &chunk[2..34];
		while let [ rest @ .., 0 ] = drive_id {
			drive_id = rest;
		}

		// Make sure we can stringify it.
		let Ok(drive_id) = std::str::from_utf8(drive_id) else { continue; };

		// We'll store the parts here in our glumped Vendor/Model array. See
		// the program's DriveVendorModel struct for more information.
		let mut vendormodel = VendorModel::default();

		// The vendor and model are separated by " - " for whatever reason, but
		// when the vendor is absent, the string will start with "- " instead.
		// Let's check for the latter first.
		if let Some(mut model) = drive_id.strip_prefix("- ") {
			model = model.trim();

			// Model is required and must fit within its maximum length.
			if (1..=MAX_MODEL_LEN).contains(&model.len()) {
				for (b, v) in vendormodel.iter_mut().skip(MAX_VENDOR_LEN).zip(model.bytes()) {
					*b = v.to_ascii_uppercase();
				}
				parsed.insert(vendormodel, offset);
			}
		}
		// This should have a form like "VENDOR - MODEL".
		else {
			let mut split = drive_id.splitn(2, " - ");
			let Some(mut vendor) = split.next() else { continue; };
			let Some(mut model) = split.next() else { continue; };
			vendor = vendor.trim();
			model = model.trim();

			// Both are required and must fit within their maximum lengths.
			if (1..=MAX_VENDOR_LEN).contains(&vendor.len()) && (1..=MAX_MODEL_LEN).contains(&model.len()) {
				for (b, v) in vendormodel.iter_mut().zip(vendor.bytes()) {
					*b = v.to_ascii_uppercase();
				}
				for (b, v) in vendormodel.iter_mut().skip(MAX_VENDOR_LEN).zip(model.bytes()) {
					*b = v.to_ascii_uppercase();
				}

				// Add it!
				parsed.insert(vendormodel, offset);
			}
		}
	}

	// Return it if we got it.
	if parsed.is_empty() { panic!("No drive offsets could be parsed."); }
	parsed
}

/// # Try Cache.
///
/// The downloaded files are cached locally in the `target` directory, but we
/// don't want to run the risk of those growing stale if they persist between
/// sessions, etc.
///
/// At the moment, cached files are used if they are less than one day old,
/// otherwise the cache is ignored and they're downloaded fresh.
fn try_cache(path: &Path) -> Option<Vec<u8>> {
	std::fs::metadata(path)
		.ok()
		.filter(Metadata::is_file)
		.and_then(|meta| meta.modified().ok())
		.and_then(|time| time.elapsed().ok().filter(|secs| secs.as_secs() < 86400))
		.and_then(|_| std::fs::read(path).ok())
}
