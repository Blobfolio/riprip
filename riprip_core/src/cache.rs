/*!
# Rip Rip Hooray: Cache
*/

use crate::RipRipError;
use fyi_msg::Msg;
use std::{
	path::{
		Path,
		PathBuf,
	},
	sync::OnceLock,
};



/// # Cache Root.
static CACHE_ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();

/// # Clean Cache.
///
/// Remove all files in the cache directory, if any.
///
/// ## Errors
///
/// This will return an error if the cache cannot be established, or any files
/// prove unreadable.
pub fn cache_clean() -> Result<(), RipRipError> {
	let root = cache_root()?;
	for e in std::fs::read_dir(root).map_err(|_| RipRipError::Cache)?.flatten() {
		if let Ok(path) = std::fs::canonicalize(e.path()) {
			if path.starts_with(root) {
				if path.is_dir() { std::fs::remove_dir_all(&path) }
				else { std::fs::remove_file(&path) }
					.map_err(|_| RipRipError::Delete(path.to_string_lossy().into_owned()))?;
			}
		}
	}
	Ok(())
}

/// # Cache Path.
pub(super) fn cache_path<P>(src: P) -> Result<PathBuf, RipRipError>
where P: AsRef<Path> {
	cache_root().map(|root| root.join(src))
}

/// # Read From Cache.
///
/// Read a file from the cache, if it exists.
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
/// Return the canonical cache root for the program.
///
/// ## Errors
///
/// This will return an error if the path cannot be determined or the directory
/// does not exist.
fn cache_root() -> Result<&'static Path, RipRipError> {
	let out = CACHE_ROOT.get_or_init(|| {
		// The base must already exist.
		let dir = std::env::current_dir().ok()?;
		if ! dir.is_dir() { return None; }

		// Our root.
		let dir = dir.join("_riprip");

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
		Msg::warning("The riprip cache directory has vanished!").eprint();
		std::fs::create_dir_all(out).map_err(|_| RipRipError::Cache)?;
		if out.is_dir() { Ok(out) }
		else { Err(RipRipError::Cache) }
	}
}
