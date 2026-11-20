//! CTDB v3 lookup and syndrome records.

use super::STRIDE;
use crate::chk::{UA, ctdb_cache_name};
use cdtoc::{CtdbLookup, Toc};
use std::{io::Read, ops::Range};

/// Maximum size of a lookup response, including fuzzy entries.
const LOOKUP_LIMIT: usize = 2 * 1024 * 1024;
/// Number of interleaved 16-bit words per stride.
const LANES: usize = STRIDE * 2;

/// A database pressing and its available reference data.
pub(super) struct Entry {
	/// CTDB's numeric entry ID.
	pub(super) id: u64,
	/// Number of submissions behind this checksum.
	pub(super) confidence: u16,
	/// CRC of the record's protected audio window.
	pub(super) disc_crc: u32,
	/// Track CRCs in the record's INDEX 01 coordinates.
	pub(super) checks: Vec<(Range<usize>, u32)>,
	/// Advertised syndromes for the first channel lane.
	pub(super) reference: Vec<u16>,
	/// HTTPS path of the full syndrome record, when available.
	pub(super) parity_url: Option<String>,
}

/// Reuse the normal verifier's cached lookup. A read-only caller can use this
/// path even when the rip cache has never been created.
pub(super) fn lookup(toc: &Toc, lengths: &[usize], fresh: bool) -> Result<Vec<Entry>, String> {
	let cached = read_lookup_cache(toc).ok().and_then(|bytes| parse(&bytes, toc, lengths).ok());
	match cached {
		Some(entries) if !fresh && entries.iter().any(|e| e.parity_url.is_some()) => Ok(entries),
		cached => request(&toc.ctdb_lookup_url(), LOOKUP_LIMIT, None)
			.and_then(|bytes| parse(&bytes, toc, lengths).map_err(str::to_owned))
			.or_else(|err| cached.ok_or(err)),
	}
}

/// Read a bounded copy of the existing checksum lookup.
fn read_lookup_cache(toc: &Toc) -> std::io::Result<Vec<u8>> {
	let path = std::env::current_dir()?.join(crate::CACHE_BASE).join(ctdb_cache_name(toc));
	let file = std::fs::File::open(path)?;
	let mut bytes = Vec::new();
	file.take(LOOKUP_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
	Ok(bytes)
}

/// Adapt cdtoc's compatible lookup records to the repair decoder's geometry.
fn parse(bytes: &[u8], toc: &Toc, lengths: &[usize]) -> Result<Vec<Entry>, &'static str> {
	if bytes.len() > LOOKUP_LIMIT { return Err("CTDB lookup exceeded the size limit"); }
	let xml = std::str::from_utf8(bytes).map_err(|_| "CTDB lookup was not UTF-8")?;
	let lookup = CtdbLookup::from_xml(xml, toc).map_err(|_| "no compatible CTDB records")?;

	let mut entries = Vec::new();
	for record in lookup.entries() {
		let settings = record.parity_settings();
		if usize::from(settings.stride().get()) != STRIDE { continue; }

		let mut checks = Vec::with_capacity(lengths.len());
		// In this lookup format zero is also used for an unavailable track CRC.
		let mut start = 0;
		for (&len, &crc) in lengths.iter().zip(record.track_checksums()) {
			let end = start + len;
			if crc != 0 { checks.push((start..end, crc)); }
			start = end;
		}

		let id = record.id();
		// CTDB's parity host is HTTP-only. Its S3 bucket also serves HTTPS.
		let parity_url = record.parity_data_url()
			.map(|_| format!("https://s3.amazonaws.com/p.cuetools.net/{id}"));
		entries.push(Entry {
			id,
			confidence: record.confidence().get(),
			disc_crc: record.album_checksum(),
			checks,
			reference: settings.syndrome().to_vec(),
			parity_url,
		});
	}

	if entries.is_empty() { return Err("no compatible CTDB records"); }
	entries.sort_by_key(|entry| std::cmp::Reverse(entry.confidence));
	Ok(entries)
}

/// Download the complete record, with rows of little-endian words per root.
pub(super) fn parity(entry: &Entry) -> Result<Vec<u16>, String> {
	let url = entry.parity_url.as_deref().ok_or("no supported parity URL")?;
	let size = entry.reference.len() * LANES * 2;
	let bytes = request(url, size, Some(size))?;
	let stride_bytes = LANES * 2;
	if bytes.is_empty() || !bytes.len().is_multiple_of(stride_bytes) {
		return Err("CTDB parity does not contain complete syndrome rows".into());
	}
	let data = words(&bytes);
	if !entry.reference.iter().zip(data.iter().step_by(LANES)).all(|(a, b)| a == b) {
		return Err("CTDB parity disagrees with the lookup syndrome".into());
	}
	Ok(data)
}

/// Parse unsigned PCM field elements without depending on host byte order.
fn words(bytes: &[u8]) -> Vec<u16> {
	bytes.as_chunks::<2>().0.iter().copied().map(u16::from_le_bytes).collect()
}

/// Fetch a bounded response from the selected lookup or parity endpoint.
fn request(url: &str, limit: usize, range: Option<usize>) -> Result<Vec<u8>, String> {
	let mut get = minreq::get(url)
		.with_header("user-agent", UA)
		.with_timeout(15)
		.with_max_headers_size(Some(32_768_usize))
		.with_max_redirects(0);
	if let Some(size) = range { get = get.with_header("range", format!("bytes=0-{}", size - 1)); }
	let response = get.send_lazy().map_err(|err| format!("CTDB request failed ({err})"))?;
	let size = match response.status_code {
		200 => None,
		206 if range.is_some() => {
			let size = response.headers.iter()
				.find(|(name, _)| name.eq_ignore_ascii_case("content-range"))
				.and_then(|(_, value)| complete_range(value, limit))
				.ok_or("unexpected CTDB parity byte range")?;
			Some(size)
		},
		404 if range.is_none() => return Err("disc is absent from CTDB".into()),
		status => return Err(format!("CTDB returned HTTP {status}")),
	};
	let mut bytes = Vec::new();
	response.take(limit as u64 + 1).read_to_end(&mut bytes)
		.map_err(|err| format!("could not read the CTDB response ({err})"))?;
	if bytes.len() > limit { return Err("CTDB response exceeded the size limit".into()); }
	if size.is_some_and(|length| length != bytes.len()) {
		return Err("CTDB parity response was truncated".into());
	}
	Ok(bytes)
}

/// Ensure a ranged response includes the entire object, even if it has fewer
/// rows than advertised. Partial or missing rows can't be used as parity.
fn complete_range(header: &str, expected_limit: usize) -> Option<usize> {
	let (last, total) = header.strip_prefix("bytes 0-")?.split_once('/')?;
	let last = last.parse::<usize>().ok()?;
	let total = total.parse::<usize>().ok()?;
	(last.checked_add(1) == Some(total) && total <= expected_limit).then_some(total)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_live_v3_attributes_and_skips_incompatible_records() {
		let xml = br#"<ctdb xmlns="http://db.cuetools.net/ns/mmd-1.0#">
			<entry confidence="8" crc32="e0a03be5" hasparity="http://p.cuetools.net/94933"
				id="94933" npar="8" stride="5880" syndrome="U5VQSKpJeu1+GUqT6KNUrw=="
				toc="0:11413:25024:45713:55220" trackcrcs="c6858467 bd788446 d479ac2e 193890d4"/>
			<entry confidence="1" crc32="9b38421f" id="4700756" npar="16" stride="5880"
				syndrome="U5VQSKpJeu1+GUqT6KNUr89AVe1jHL/mwiMSSBI9X2w="
				toc="0:11413:25024:45713:55220" trackcrcs="c6858467 bd788446 dc379f45 193890d4"/>
			<entry confidence="12" crc32="00000001" id="2" npar="8" stride="5881"
				syndrome="U5VQSKpJeu1+GUqT6KNUrw==" toc="0:11413:25024:45713:55220"
				trackcrcs="c6858467 bd788446 d479ac2e 193890d4"/>
		</ctdb>"#;
		// The local disc has a pregap and a data session outside the audio image.
		let toc = Toc::from_parts(vec![250, 11_663, 25_274, 45_963], Some(66_870), 70_000).unwrap();
		let lengths: Vec<_> = toc.audio_tracks().map(|track| track.duration().samples() as usize).collect();
		let entries = parse(xml, &toc, &lengths).unwrap();
		assert_eq!(entries.len(), 2, "unsupported stride is skipped and checksum-only entries are retained");
		let entry = &entries[0];
		assert_eq!(entry.id, 94_933, "confidence order");
		assert_eq!(entry.disc_crc, 0xe0a0_3be5, "disc CRC");
		assert_eq!(entry.reference[0], 0x9553, "little-endian syndrome");
		assert_eq!(entry.checks[0], (0..11_413 * 588, 0xc685_8467), "track CRC range");
		assert_eq!(entry.parity_url.as_deref(), Some("https://s3.amazonaws.com/p.cuetools.net/94933"),
			"verified S3 HTTPS host");
		assert!(entries[1].parity_url.is_none(), "checksum-only entry");
	}
}
