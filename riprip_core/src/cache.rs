/*!
# Rip Rip Hooray: Cache
*/

use crate::{
	CACHE_BASE,
	RipRipError,
};
use fyi_msg::Msg;
use std::{
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
	let src = cache_path(src)?;
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
	let dst = cache_path(dst)?;
	write_atomic::write_file(&dst, data)
		.map_err(|_| RipRipError::Write(dst.to_string_lossy().into_owned()))
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
