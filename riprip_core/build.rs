/*!
# Rip Rip Hooray: Build

This just pre-parses the list of drive offsets into a reasonable structure that
can be embedded directly in the code.
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
type VendorModel = [u8; 24];

/// # Can I Use? publishes their data here.
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

	let mut out = format!(
		r#"
/// # Drive Offsets.
const DRIVE_OFFSETS: [(DriveVendorModel, ReadOffset); {}] = ["#,
		nice.len(),
	);

	for chunk in nice.chunks(256) {
		write!(&mut out, "\n\t{}", chunk.join(" ")).expect("Failed to write string.");
	}

	out.push_str("\n];\n");

	// Save it.
	let dst = out_path("drive-offsets.rs");
	File::create(dst)
		.and_then(|mut f| f.write_all(out.as_bytes()).and_then(|_| f.flush()))
		.expect("Unable to save drive offsets.");
}



/// # Download/Cache Raw JSON.
fn fetch() -> Vec<u8> {
	// Is it cached?
	let cache = out_path("DriveOffsets.bin");
	if let Some(x) = try_cache(&cache) { return x; }

	fetch_remote(&cache).expect("Unable to download AccurateRip drive offsets.")
}

/// # Fetch Remote.
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
		if offset == 0 { continue; } // There's no point detecting no offset.

		// We don't support insane offsets, just crazy ones.
		if ! (MIN_OFFSET..=MAX_OFFSET).contains(&offset) { continue; }

		// The drive ID is allotted 32 bytes, but unused space remains null.
		// Let's trim those off real quick.
		let mut drive_id = &chunk[2..34];
		while let [ rest @ .., 0 ] = drive_id {
			drive_id = rest;
		}

		// Make sure we can stringify it.
		let Ok(drive_id) = std::str::from_utf8(drive_id) else { continue; };

		// We'll store the parts here in our glumped Vendor/Model array.
		let mut vendormodel = VendorModel::default();

		// The vendor and model are separated by " - ", but if the vendor is
		// absent, the string will start with "- ". Let's check the latter
		// first.
		if let Some(mut model) = drive_id.strip_prefix("- ") {
			model = model.trim();

			if (1..=MAX_MODEL_LEN).contains(&model.len()) {
				for (b, v) in vendormodel.iter_mut().skip(MAX_VENDOR_LEN).zip(model.bytes()) {
					*b = v.to_ascii_uppercase();
				}
				parsed.insert(vendormodel, offset);
			}
		}
		else {
			let mut split = drive_id.splitn(2, " - ");
			let Some(mut vendor) = split.next() else { continue; };
			let Some(mut model) = split.next() else { continue; };
			vendor = vendor.trim();
			model = model.trim();

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

	if parsed.is_empty() { panic!("No drive offsets could be parsed."); }
	parsed
}

/// # Try Cache.
///
/// The downloaded files are cached locally in the `target` directory, but we
/// don't want to run the risk of those growing stale if they persist between
/// sessions, etc.
///
/// At the moment, cached files are used if they are less than an hour old,
/// otherwise the cache is ignored and they're downloaded fresh.
fn try_cache(path: &Path) -> Option<Vec<u8>> {
	std::fs::metadata(path)
		.ok()
		.filter(Metadata::is_file)
		.and_then(|meta| meta.modified().ok())
		.and_then(|time| time.elapsed().ok().filter(|secs| secs.as_secs() < 3600))
		.and_then(|_| std::fs::read(path).ok())
}
