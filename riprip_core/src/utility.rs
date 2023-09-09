/*!
# Rip Rip Hooray: Encode/Decode
*/

use crate::{
	CACHE_BASE,
	RipRipError,
};
use fyi_msg::Msg;
use std::{
	io::{
		Read,
		Write,
	},
	path::{
		Path,
		PathBuf,
	},
	sync::OnceLock,
};



/// # Cache Root.
///
/// This will ultimately hold `CWD/CACHE_BASE`.
static CACHE_ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();



/// # Cache Path.
///
/// Glue `src` onto the cache path and return it.
///
/// ## Errors
///
/// This will return an error if the cache root cannot be established.
pub(super) fn cache_path<P>(src: P) -> Result<PathBuf, RipRipError>
where P: AsRef<Path> {
	cache_root().map(|root| root.join(src))
}

/// # Read From Cache.
///
/// Read a file from the cache, if it exists.
///
/// Note: the path should be _relative_ to the cache root.
///
/// ## Errors
///
/// This will return an error if the cache root cannot be established, but will
/// otherwise simply return `None` if there are problems with the file.
pub(super) fn cache_read<P>(src: P) -> Result<Option<Vec<u8>>, RipRipError>
where P: AsRef<Path> {
	let src = src.as_ref();
	validate_cache_path(src)?;
	Ok(std::fs::read(src).ok().filter(|v| ! v.is_empty()))
}

/// # Write to Cache.
///
/// Write a file to the cache, replacing the original if it exists.
///
/// Note: the path should be _relative_ to the cache root.
///
/// ## Errors
///
/// This will return an error if the cache root cannot be established or if
/// there are problems writing this specific file.
pub(super) fn cache_write<P>(dst: P, data: &[u8]) -> Result<(), RipRipError>
where P: AsRef<Path> {
	let dst = dst.as_ref();
	validate_cache_path(dst)?;
	write_atomic::write_file(dst, data)
		.map_err(|_| RipRipError::Write(dst.to_string_lossy().into_owned()))
}

/// # Zstd Decode.
///
/// Return a decompressed copy of `raw`, or `None` if the operation fails.
pub(crate) fn zstd_decode(raw: &[u8]) -> Option<Vec<u8>> {
	let mut out = Vec::with_capacity(raw.len() * 2);
	let mut decoder = zstd::stream::Decoder::new(raw).ok()?;
	decoder.read_to_end(&mut out).ok()?;

	// Make sure we have something.
	if out.is_empty() { None }
	else { Some(out) }
}

/// # Zstd Encode.
///
/// Return a copy of the `raw` compressed with default-level zstd. If there is
/// any sort of problem, `None` will be returned instead.
///
/// Compression is used for the serialized state data as it can get rather
/// large.
pub(crate) fn zstd_encode(raw: &[u8]) -> Option<Vec<u8>> {
	let mut encoder = zstd::stream::Encoder::new(
		Vec::with_capacity(raw.len().wrapping_div(2)),
		zstd::DEFAULT_COMPRESSION_LEVEL,
	).ok()?;
	encoder.write_all(raw).ok()?;
	let out = encoder.finish().ok()?;

	// Make sure there's something.
	if out.is_empty() { None }
	else { Some(out) }
}



/// # Cache Root.
///
/// Return the canonical cache root for the program, creating it if it doesn't
/// already exist.
///
/// ## Errors
///
/// This will return an error if the path cannot be determined or the current
/// working directory does not exist.
fn cache_root() -> Result<&'static Path, RipRipError> {
	let out = CACHE_ROOT.get_or_init(|| {
		// The base must already exist.
		let dir = std::env::current_dir().ok()?;
		if ! dir.is_dir() { return None; }

		// Our root.
		let dir = dir.join(CACHE_BASE);

		// Make it if necessary.
		if ! dir.is_dir() {
			std::fs::create_dir_all(&dir).ok()?;
		}

		// Make sure it is really there.
		std::fs::canonicalize(dir).ok()
	})
		.as_deref()
		.ok_or(RipRipError::Cache)?;

	if out.is_dir() { Ok(out) }
	// It seems to have vanished… try to recreate it.
	else {
		Msg::warning(format!("The {CACHE_BASE} cache directory has vanished!")).eprint();
		std::fs::create_dir_all(out).map_err(|_| RipRipError::Cache)?;
		if out.is_dir() { Ok(out) }
		else { Err(RipRipError::Cache) }
	}
}

/// # Validate Cache Path.
///
/// Make sure a path is in the cache root.
fn validate_cache_path(src: &Path) -> Result<(), RipRipError> {
	let root = cache_root()?.to_string_lossy();
	let src = src.to_string_lossy();
	src.strip_prefix(root.as_ref())
		.and_then(|r|
			if 1 < r.len() { Some(()) }
			else { None }
		)
		.ok_or_else(|| RipRipError::CachePath(src.into_owned()))
}



#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn t_zstd() {
		let test = b"Advertising may be described as the science of arresting the human intelligence long enough to get money from it.";

		// Encode it.
		let enc = zstd_encode(test).expect("Failed to encode pithy quote.");
		assert!(enc.len() < test.len(), "Encoding sucks.");

		// Decode it.
		let dec = zstd_decode(&enc).expect("Failed to decode pithy quote.");
		assert_eq!(dec, test, "Decoded quote does not match the original!");
	}
}
