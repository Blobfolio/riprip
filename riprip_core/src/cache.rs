/*!
# Rip Rip Hooray: Encode/Decode
*/

use cdtoc::{
	Toc,
	Track,
};
use crate::{
	CACHE_BASE,
	CACHE_SCRATCH,
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
use tempfile::NamedTempFile;



/// # Cache Root.
///
/// This will ultimately hold `CWD/CACHE_BASE`.
static CACHE_ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();

/// # Cache Prefix.
///
/// The formatted CDDB ID for the current disc.
static CACHE_PREFIX: OnceLock<String> = OnceLock::new();



/// # Cache Writer.
///
/// This is a simple wrapper around `Tempfile` that abstracts away the
/// particulars, minimizing code repetition.
pub(super) struct CacheWriter<'a> {
	dst: &'a Path,
	tmp: NamedTempFile,
}

impl<'a> CacheWriter<'a> {
	/// # New Writer.
	///
	/// Create a new writer for the given destination path.
	///
	/// The temporary file is placed in the same parent directory as the
	/// destination to ensure writeability. If that directory does not yet
	/// exist it will be automatically created.
	///
	/// ## Errors
	///
	/// This will bubble up any I/O-related errors.
	pub(super) fn new(dst: &'a Path) -> Result<Self, RipRipError> {
		// The destination doesn't have to exist, but can't be a directory.
		if dst.is_dir() {
			return Err(RipRipError::CachePath(dst.to_string_lossy().into_owned()));
		}

		// It must have a parent directory.
		let parent = dst.parent()
			.ok_or_else(|| RipRipError::CachePath(dst.to_string_lossy().into_owned()))?;

		// If that doesn't exist, try to create it.
		if ! parent.is_dir() {
			std::fs::create_dir_all(parent)
				.map_err(|_| RipRipError::CachePath(dst.to_string_lossy().into_owned()))?;
		}

		// Make a tempfile.
		let tmp = tempfile::Builder::new().tempfile_in(parent)
			.map_err(|_| RipRipError::CachePath(dst.to_string_lossy().into_owned()))?;

		// We should be good!
		Ok(Self { dst, tmp })
	}

	/// # Writer Reference.
	///
	/// Return a mutable reference to the underlying file writer.
	pub(super) fn writer(&mut self) -> &mut NamedTempFile { &mut self.tmp }

	/// # Finish it off!
	///
	/// Flush the data (just in case) and permanently save the contents to
	/// `self.dst`.
	pub(super) fn finish(mut self) -> Result<(), RipRipError> {
		use std::io::Write;

		// Flush for good measure.
		self.tmp.flush()
			.map_err(|_| RipRipError::CachePath(self.dst.to_string_lossy().into_owned()))?;

		self.tmp.persist(self.dst)
			.map(|_| ())
			.map_err(|_| RipRipError::CachePath(self.dst.to_string_lossy().into_owned()))
	}
}



/// # Cache Path.
///
/// Glue `src` onto the cache root and return the resulting path.
///
/// ## Errors
///
/// This will return an error if the cache root cannot be established.
pub(super) fn cache_path<P>(src: P) -> Result<PathBuf, RipRipError>
where P: AsRef<Path> {
	cache_root().map(|root| root.join(src))
}

/// # Cache Prefix.
///
/// All of the file names are prefixed with the disc's CDDB ID. This is
/// unnecessarily complicated to generate — especially for such a tiny payload
/// — so we'll do it once.
///
/// This also ensures we're handling the value consistently.
pub(super) fn cache_prefix(toc: &Toc) -> &'static str {
	CACHE_PREFIX.get_or_init(|| toc.cddb_id().to_string())
}

/// # State Path.
///
/// Return the file path to save the state data to.
///
/// Paths are prefixed with a CRC32 hash of the table of contents for basic
/// collision mitigation. The state from track 2 from one disc, for example,
/// shouldn't override the state from a different disc's track 2.
///
/// ## Errors
///
/// This will return an error if there are problems determining the cache
/// location.
pub(crate) fn state_path(toc: &Toc, track: Track) -> Result<PathBuf, RipRipError> {
	cache_path(format!(
		"{CACHE_SCRATCH}/{}__{:02}.state",
		cache_prefix(toc),
		track.number()
	))
}

/// # Track Path.
///
/// Return the file path to save the exported track to. To keep things
/// predictable, this is simply the CDDB ID and two-digit track number.
///
/// ## Errors
///
/// This will return an error if there are problems determining the cache
/// location.
pub(crate) fn track_path(toc: &Toc, track: Track) -> Result<PathBuf, RipRipError> {
	cache_path(format!("{}__{:02}.wav", cache_prefix(toc), track.number()))
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
