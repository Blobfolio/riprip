//! Install verified PCM at the normal WAV paths and recognize it on resume.

use super::{Audio, pcm_frame, repair::Repair};
use crate::{CACHE_SCRATCH, KillSwitch, SavedRips, cache::{cache_path, cache_prefix, track_path}};
use cdtoc::{Toc, Track};
use std::{fs, io::{self, Read, Seek, SeekFrom}, ops::Range, path::{Path, PathBuf}};

/// Bind a verified export to its bytes and the original disc and track.
const MARKER_MAGIC: [u8; 8] = *b"RRCTDB01";

/// Serialize WAV exports and repair publication for one disc.
pub(in crate::rip) fn lock(toc: &Toc) -> Result<fs::File, crate::RipRipError> {
	let path = cache_path(format!("{CACHE_SCRATCH}/{}__export.lock", cache_prefix(toc)))?;
	let error = |_| crate::RipRipError::CachePath(path.to_string_lossy().into_owned());
	if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(error)?; }
	let lock = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false)
		.open(&path).map_err(error)?;
	lock.lock().map_err(error)?;
	Ok(lock)
}

/// Confidence of a previously repaired WAV whose bytes still match the export.
pub(in crate::rip) fn confidence(toc: &Toc, track: Track, wav: &Path) -> Option<u16> {
	let mut marker = [0; 18];
	let mut file = fs::File::open(marker_path(toc, track).ok()?).ok()?;
	file.read_exact(&mut marker).ok()?;
	if marker[..8] != MARKER_MAGIC || marker[8..12] != track_id(toc, track).to_le_bytes() {
		return None;
	}
	let crc = u32::from_le_bytes(marker[12..16].try_into().ok()?);
	let confidence = u16::from_le_bytes(marker[16..18].try_into().ok()?);
	(wave_crc(wav).ok()? == crc).then_some(confidence)
}

/// Write every affected or missing WAV before replacing any normal output.
pub(super) fn publish(toc: &Toc, killed: KillSwitch, audio: &Audio, repair: &Repair,
	window: Range<usize>, confidence: u16, refresh: &[u8]) -> Result<SavedRips, String> {
	let _lock = lock(toc).map_err(|err| err.to_string())?;
	let first = toc.audio_tracks().next().ok_or("no audio tracks")?;
	let path = track_path(toc, first).map_err(|err| err.to_string())?;
	let parent = path.parent().ok_or("missing output directory")?;
	let stage = tempfile::Builder::new().prefix(".ctdb-stage-").tempdir_in(parent)
		.map_err(|err| format!("cannot stage verified repair ({err})"))?;
	let mut tracks = SavedRips::new();
	let mut replacements = Vec::new();
	let mut patches = repair.corrections.iter().peekable();
	for (idx, track) in toc.audio_tracks().enumerate() {
		if killed.killed() { return Err("export cancelled".into()); }
		let start = audio.boundaries[idx];
		let end = audio.boundaries[idx + 1];
		let dst = track_path(toc, track).map_err(|err| err.to_string())?;
		let changed = patches.peek().is_some_and(|&&(at, _)| at < end * 2) ||
			audio.overlap_changes.get(audio.overlap_changes.partition_point(|&at| at < start))
				.is_some_and(|&at| at < end) || refresh.contains(&track.number());
		let covered = start.max(window.start)..end.min(window.end);
		if covered.is_empty() { continue; }
		if !changed && {
			let source = &audio.pcm[2 * covered.start..2 * covered.end];
			matches_wave(&dst, end - start, covered.start - start, source).unwrap_or(false)
		} { continue; }

		let src = stage.path().join(format!("{idx}.wav"));
		let frames = audio.pcm[2 * start..2 * end].as_chunks::<2>().0.iter().enumerate().map(|(i, words)| {
			let mut frame = *words;
			for (channel, word) in frame.iter_mut().enumerate() {
				if patches.peek().is_some_and(|&&(at, _)| at == 2 * (start + i) + channel) {
					*word ^= patches.next().expect("checked correction").1;
				}
			}
			pcm_frame(frame)
		});
		crate::rip::data::write_wave(&src, frames).map_err(|err| err.to_string())?;
		let crc = wave_crc(&src).map_err(|err| format!("cannot fingerprint staged WAV ({err})"))?;
		let staged_marker = stage.path().join(format!("{idx}.info"));
		let mut marker = Vec::from(MARKER_MAGIC);
		marker.extend(track_id(toc, track).to_le_bytes());
		marker.extend(crc.to_le_bytes());
		marker.extend(confidence.to_le_bytes());
		fs::write(&staged_marker, marker).map_err(|err| format!("cannot stage repair metadata ({err})"))?;
		let marker_dst = marker_path(toc, track).map_err(|err| err.to_string())?;
		replacements.push((src, dst.clone()));
		replacements.push((staged_marker, marker_dst));
		tracks.insert(track.number(), (dst, None, Some(confidence)));
	}

	install(stage, &replacements, killed)?;
	Ok(tracks)
}

/// Confirm a retained WAV contains the verified slice before skipping its reads.
fn matches_wave(path: &Path, frames: usize, start: usize, pcm: &[u16]) -> io::Result<bool> {
	use crate::rip::data::WAVE_HEADER;
	let mut wav = fs::File::open(path)?;
	let mut header = [0; WAVE_HEADER.len()];
	wav.read_exact(&mut header)?;
	let data_len = u32::try_from(frames * 4).map_err(io::Error::other)?;
	if header[..4] != WAVE_HEADER[..4] || header[8..40] != WAVE_HEADER[8..40] ||
		header[40..] != data_len.to_le_bytes() ||
		wav.metadata()?.len() != u64::from(data_len) + header.len() as u64
	{
		return Ok(false);
	}
	wav.seek(SeekFrom::Current(i64::try_from(start * 4).map_err(io::Error::other)?))?;
	let mut bytes = [0; 16_384];
	for block in pcm.chunks(bytes.len() / 2) {
		let bytes = &mut bytes[..block.len() * 2];
		wav.read_exact(bytes)?;
		if !bytes.as_chunks::<2>().0.iter().zip(block).all(|(&got, &word)| got == word.to_le_bytes()) {
			return Ok(false);
		}
	}
	Ok(true)
}

/// Keep backups until all same-filesystem replacements have succeeded.
fn install(stage: tempfile::TempDir, replacements: &[(PathBuf, PathBuf)],
	killed: KillSwitch) -> Result<(), String> {
	let backups = replacements.iter().enumerate().map(|(i, (_, dst))| {
		match fs::symlink_metadata(dst) {
			Ok(meta) if meta.is_file() => {
				let backup = stage.path().join(format!("{i}.backup"));
				fs::hard_link(dst, &backup)
					.or_else(|_| fs::copy(dst, &backup).map(|_| ()))
					.map(|()| Some(backup))
			},
			Ok(_) => Err(io::Error::other("existing destination is not a regular file")),
			Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
			Err(err) => Err(err),
		}.map_err(|err| format!("cannot stage replacement for {} ({err})", dst.display()))
	}).collect::<Result<Vec<_>, _>>()?;
	if killed.killed() { return Err("export cancelled".into()); }
	for (i, (src, dst)) in replacements.iter().enumerate() {
		if let Err(err) = fs::rename(src, dst) {
			let failed = format!("could not replace {} ({err})", dst.display());
			let mut rollback_failed = false;
			for ((_, path), backup) in replacements[..i].iter().zip(&backups).rev() {
				let restored = backup.as_ref().map_or_else(|| fs::remove_file(path),
					|backup| fs::rename(backup, path));
				rollback_failed |= restored.is_err();
			}
			if rollback_failed {
				return Err(format!("{failed}. Original files for recovery are in {}", stage.keep().display()));
			}
			return Err(failed);
		}
	}
	Ok(())
}

/// Fingerprint one complete WAV, including the audio header.
fn wave_crc(path: &Path) -> io::Result<u32> {
	let mut wav = fs::File::open(path)?;
	let mut crc = crc32fast::Hasher::new();
	let mut buffer = [0; 16_384];
	loop {
		let len = wav.read(&mut buffer)?;
		if len == 0 { return Ok(crc.finalize()); }
		crc.update(&buffer[..len]);
	}
}

/// Use the same disc and track numbering as the rest of the rip cache.
fn marker_path(toc: &Toc, track: Track) -> Result<PathBuf, crate::RipRipError> {
	cache_path(format!("{CACHE_SCRATCH}/{}__{:02}.ctdb", cache_prefix(toc), track.number()))
}

/// Disambiguate discs that share a cache name.
fn track_id(toc: &Toc, track: Track) -> u32 {
	let mut crc = crc32fast::Hasher::new();
	crc.update(toc.to_string().as_bytes());
	crc.update(&[track.number()]);
	crc.finalize()
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::{Arc, LazyLock, atomic::AtomicBool};

	/// An install failure restores paths that were already replaced.
	#[test]
	fn rolls_back_a_replacement_batch() {
		static ACTIVE: LazyLock<Arc<AtomicBool>> = LazyLock::new(|| Arc::new(AtomicBool::new(false)));
		let root = tempfile::tempdir().unwrap();
		let stage = tempfile::tempdir_in(root.path()).unwrap();
		let first = root.path().join("01.wav");
		let second = root.path().join("02.wav");
		let new = stage.path().join("new.wav");
		let missing = stage.path().join("missing.wav");
		fs::write(&first, b"original").unwrap();
		fs::write(&second, b"original two").unwrap();
		fs::write(&new, b"verified").unwrap();
		let result = install(stage, &[(new, first.clone()), (missing, second.clone())],
			KillSwitch::from(&*ACTIVE));
		assert!(result.is_err(), "the second replacement fails");
		assert_eq!(fs::read(first).unwrap(), b"original", "the first replacement rolls back");
		assert_eq!(fs::read(second).unwrap(), b"original two", "the second WAV is preserved");
	}
}
