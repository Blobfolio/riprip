/*!
# Rip Rip Hooray: CD-Text.
*/

use std::collections::HashMap;

use crate::cdtext::CDTextKind;

use super::language::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub(super) enum Field {
    Title = 0x80,
    Performer = 0x81,
    Songwriter = 0x82,
    Composer = 0x83,
    Arranger = 0x84,
    Message = 0x85,
    DiscId = 0x86,
    Genre = 0x87,
    /// UPC/EAN at the disc level or ISRC for individual tracks.
    UpcEan = 0x8E,
}

impl TryFrom<u8> for Field {
    type Error = u8;

    #[expect(unsafe_code, reason = "For FFI.")]
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x80..=0x87 | 0x8E => unsafe { Ok(std::mem::transmute(value)) },
            unmapped => Err(unmapped),
        }
    }
}

impl From<CDTextKind> for Field {
    fn from(value: CDTextKind) -> Self {
        match value {
            CDTextKind::Arranger => Self::Arranger,
            CDTextKind::Barcode => Self::UpcEan,
            CDTextKind::Composer => Self::Composer,
            CDTextKind::Isrc => Self::UpcEan,
            CDTextKind::Message => Self::Message,
            CDTextKind::Performer => Self::Performer,
            CDTextKind::Songwriter => Self::Songwriter,
            CDTextKind::Title => Self::Title,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Encoding {
    /// ISO-8859-1 (8 bit), Latin-1
    Iso8859_1 = 0x00,
    /// ASCII (7 bit)
    Ascii = 0x01,
    /// Shift-JIS (double byte)
    ShiftJis = 0x80,
}

impl TryFrom<u8> for Encoding {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::Iso8859_1),
            0x01 => Ok(Self::Ascii),
            0x80 => Ok(Self::ShiftJis),
            unmapped => Err(unmapped),
        }
    }
}

impl Encoding {
    /// Converts raw text pack bytes into a valid Rust UTF-8 String.
    pub(super) fn decode(&self, bytes: &[u8]) -> String {
        match self {
            Self::Iso8859_1 | Self::Ascii => {
                // Try to parse directly as UTF-8/ASCII first without looping.
                match std::str::from_utf8(bytes) {
                    Ok(valid_str) => valid_str.to_string(),
                    Err(_) => bytes.iter().map(|&b| b as char).collect(),
                }
            }
            Self::ShiftJis => encoding_rs::SHIFT_JIS.decode(bytes).0.into_owned(),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
enum GenreCode {
    #[default]
    Unused = 0,
    Undefined = 1,
    AdultContemporary = 2,
    AlternativeRock = 3,
    Childrens = 4,
    Classical = 5,
    ChristContemporary = 6,
    Country = 7,
    Dance = 8,
    EasyListening = 9,
    Erotic = 10,
    Folk = 11,
    Gospel = 12,
    HipHop = 13,
    Jazz = 14,
    Latin = 15,
    Musical = 16,
    NewAge = 17,
    Opera = 18,
    Operetta = 19,
    Pop = 20,
    Rap = 21,
    Reggae = 22,
    Rock = 23,
    RhythmAndBlues = 24,
    SoundEffects = 25,
    Soundtrack = 26,
    SpokenWord = 27,
    WorldMusic = 28,
}

impl TryFrom<u8> for GenreCode {
    type Error = u8;

    #[expect(unsafe_code, reason = "For FFI.")]
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0..=28 => unsafe { Ok(std::mem::transmute(value)) },
            unmapped => Err(unmapped),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct LanguageLayer {
    pub first_track: u8,
    pub last_track: u8,
    pub language: Language,
    pub catalog: HashMap<(Field, u8), String>,
}

impl LanguageLayer {
    /// Returns the album title with the leading artist name and any extra spacing stripped out.
    pub(super) fn album_title(&self) -> Option<&str> {
        let title = self.catalog.get(&(Field::Title, 0))?;
        title
            .strip_prefix(self.catalog.get(&(Field::Performer, 0))?)
            .map(|s| s.trim_start())
            .or(Some(title))
    }

    /// Returns the genre for a specific track, skipping the initial binary genre code byte.
    pub(super) fn genre(&self, track: u8) -> Option<&str> {
        self.catalog
            .get(&(Field::Genre, track))
            .and_then(|raw_string| {
                let mut chars = raw_string.char_indices();
                chars.next()?;

                let split_idx = chars.next().map(|(idx, _)| idx).unwrap_or(raw_string.len());

                Some(&raw_string[split_idx..])
            })
    }

    pub(super) fn genre_code(&self) -> Option<GenreCode> {
        let genre_str = self.catalog.get(&(Field::Genre, 0))?;
        let bytes = genre_str.as_bytes();

        // The very first byte is our binary code.
        let raw_code = *bytes.first()?;

        GenreCode::try_from(raw_code).ok()
    }
}

#[derive(Debug, Default)]
pub(super) struct Metadata {
    pub layers: Vec<LanguageLayer>,
}

#[derive(Debug)]
pub(super) enum Error {
    InvalidPack,
    InvalidEncoding,
    Unsupported,
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
struct SizeInfo {
    pub char_code: u8,
    pub first_track: u8,
    pub last_track: u8,
    pub copyright: u8, // 3: CD-TEXT is copyrighted, 0: no copyright on CD-TEXT
    pub pack_counts: [u8; 16], // 16 pack types (0x80 through 0x8F)
    pub last_seq: [u8; 8], // Last sequence number for blocks 0..7
    pub lang_code: [u8; 8], // Language code for blocks 0..7
}

impl SizeInfo {
    fn from_bytes(bytes: &[u8]) -> Self {
        assert!(bytes.len() >= 36);

        let mut pack_counts = [0u8; 16];
        pack_counts.copy_from_slice(&bytes[4..20]);

        let mut last_seq = [0u8; 8];
        last_seq.copy_from_slice(&bytes[20..28]);

        let mut lang_code = [0u8; 8];
        lang_code.copy_from_slice(&bytes[28..36]);

        Self {
            char_code: bytes[0],
            first_track: bytes[1],
            last_track: bytes[2],
            copyright: bytes[3],
            pack_counts,
            last_seq,
            lang_code,
        }
    }
}

impl Metadata {
    pub(super) fn parse(buf: &[u8]) -> Result<Option<Self>, Error> {
        if buf.len() < 4 {
            return Ok(None);
        }

        // Skip the header.
        let pack_data = &buf[4..];

        Self::parse_packs(pack_data)
    }

    fn parse_packs(pack_data: &[u8]) -> Result<Option<Self>, Error> {
        const PACK_LEN: usize = 18;
        const PACK_HEADER_LEN: usize = 4;
        const PACK_PAYLOAD_LEN: usize = 12;

        #[derive(Debug, Clone, Default)]
        struct Block {
            pub buffer: HashMap<(u8, u8), Vec<u8>>,
        }

        #[derive(Debug, Default)]
        struct Context {
            pub text_buf: Vec<u8>,
            pub language_blocks: Vec<Block>,
        }

        impl Context {
            pub(super) fn handle_pack(&mut self, pack: &[u8]) -> Result<(), Error> {
                let header = &pack[0..PACK_HEADER_LEN];
                let payload = &pack[PACK_HEADER_LEN..PACK_HEADER_LEN + PACK_PAYLOAD_LEN];

                let (id1, id2, id3, id4) = (header[0], header[1], header[2], header[3]);

                let pack_type = id1;
                let is_extension = (id2 & 0x80) != 0; // Extension Flag (0 = normal, 1 = extension)
                if is_extension {
                    return Err(Error::Unsupported);
                }
                let mut track_number = id2 & 0x7F;
                let sequence_number = id3;
                let block_id = (id4 >> 4) & 0x07; // Bits 4-6 define the language block ID.
                                                  // let char_pos = id4 & 0x0f;

                self.language_blocks
                    .resize(block_id as usize + 1, Block::default());

                // println!("{:?} {} {} {} {} {}", String::from_utf8_lossy(payload), pack_type, sequence_number, block_id, track_number, char_pos);

                let is_text = pack_type != 0x8F;
                if is_text {
                    let char_pos = id4 & 0x0f;
                    let is_double_byte = (id4 & 0x80) != 0;
                    if !is_double_byte {
                        for b in payload {
                            if *b == 0x00 {
                                if !self.text_buf.is_empty() {
                                    let key = (pack_type, track_number);
                                    self.language_blocks[block_id as usize]
                                        .buffer
                                        .insert(key, self.text_buf.clone());
                                    self.text_buf.clear();
                                }
                                track_number += 1;
                            } else if *b == b'\t' {
                                // Handle repetition.
                                let last_key = (pack_type, track_number.saturating_sub(1));
                                let cloned_buf = self.language_blocks[block_id as usize]
                                    .buffer
                                    .get(&last_key)
                                    .cloned();
                                if let Some(buf) = cloned_buf {
                                    let key = (pack_type, track_number);
                                    self.language_blocks[block_id as usize]
                                        .buffer
                                        .insert(key, buf);
                                }
                            } else {
                                self.text_buf.push(*b);
                            }
                        }
                    } else {
                        todo!()
                    }
                } else {
                    let key = (pack_type, 0);
                    let buffer = self.language_blocks[block_id as usize]
                        .buffer
                        .entry(key)
                        .or_insert(Default::default());
                    buffer.extend_from_slice(payload);
                }
                Ok(())
            }
        }

        /// Validates a raw 18-byte CD-Text pack using its trailing 2-byte CRC.
        fn is_pack_valid(pack: &[u8]) -> bool {
            use crc::{Algorithm, Crc};

            // Define the exact CD-Text CRC-16 specification parameters.
            const CDTEXT_CRC: Algorithm<u16> = Algorithm {
                width: 16,
                poly: 0x1021,
                init: 0x0000,
                refin: false,
                refout: false,
                xorout: 0xFFFF,
                check: 0x2B8C,
                residue: 0x0000,
            };

            const ENGINE: Crc<u16> = Crc::<u16>::new(&CDTEXT_CRC);

            // Extract the expected CRC from the pack.
            let expected_crc = u16::from_be_bytes([pack[16], pack[17]]);

            ENGINE.checksum(&pack[0..16]) == expected_crc
        }

        let mut context = Context::default();
        for pack in pack_data.chunks_exact(PACK_LEN) {
            if !is_pack_valid(pack) {
                return Err(Error::InvalidPack);
            }
            context.handle_pack(pack).unwrap();
        }

        let mut metadata = Self::default();
        for (i, block) in context.language_blocks.iter().enumerate() {
            let slice = block.buffer.get(&(0x8F, 0)).unwrap();
            let size_info = SizeInfo::from_bytes(slice);
            // dbg!(size_info);
            let language = Language::try_from(size_info.lang_code[i]).unwrap();
            let encoding = Encoding::try_from(size_info.char_code).unwrap();

            let mut layer = LanguageLayer::default();
            layer.first_track = size_info.first_track;
            layer.last_track = size_info.last_track;
            layer.language = language;
            for (&(pack_type, track), buf) in block.buffer.iter() {
                if let Ok(field) = Field::try_from(pack_type) {
                    let s = encoding.decode(&buf);
                    layer.catalog.insert((field, track), s.clone());
                }
            }
            metadata.layers.push(layer);
        }

        Ok(Some(metadata))
    }
}

#[cfg(test)]
mod test {
    use super::{Field, LanguageLayer, Metadata};

    fn dump(metadata: &Metadata) -> String {
        use std::fmt::Write;

        fn dump_field(
            out: &mut String,
            label: &str,
            layer: &LanguageLayer,
            field: Field,
            track: u8,
        ) {
            if let Some(val) = layer.catalog.get(&(field, track)) {
                writeln!(out, "\t{}: {}", label, val).unwrap();
            }
        }

        let mut out = String::new();

        writeln!(&mut out).unwrap();

        for (idx, layer) in metadata.layers.iter().enumerate() {
            writeln!(&mut out, "Language {} '{:?}':", idx, layer.language).unwrap();
            writeln!(&mut out, "CD-TEXT for Disc:").unwrap();

            dump_field(&mut out, "TITLE", layer, Field::Title, 0);
            dump_field(&mut out, "PERFORMER", layer, Field::Performer, 0);
            dump_field(&mut out, "SONGWRITER", layer, Field::Songwriter, 0);
            dump_field(&mut out, "COMPOSER", layer, Field::Composer, 0);
            dump_field(&mut out, "MESSAGE", layer, Field::Message, 0);
            dump_field(&mut out, "ARRANGER", layer, Field::Arranger, 0);
            dump_field(&mut out, "UPC_EAN", layer, Field::UpcEan, 0);
            if let Some(genre) = layer.genre(0) {
                writeln!(&mut out, "\tGENRE: {}", genre).unwrap();
            }
            dump_field(&mut out, "DISC_ID", layer, Field::DiscId, 0);
            if let Some(genre_code) = layer.genre_code().map(|c| format!("{} ({:?})", c as u8, c)) {
                writeln!(&mut out, "\tGENRE_CODE: {}", genre_code).unwrap();
            }

            for track in layer.first_track..=layer.last_track {
                writeln!(&mut out, "CD-TEXT for Track {:2}:", track).unwrap();

                dump_field(&mut out, "TITLE", layer, Field::Title, track);
                dump_field(&mut out, "PERFORMER", layer, Field::Performer, track);
                dump_field(&mut out, "SONGWRITER", layer, Field::Songwriter, track);
                dump_field(&mut out, "COMPOSER", layer, Field::Composer, track);
                dump_field(&mut out, "MESSAGE", layer, Field::Message, track);
                dump_field(&mut out, "ARRANGER", layer, Field::Arranger, track);
                dump_field(&mut out, "ISRC", layer, Field::UpcEan, track);
            }

            writeln!(&mut out).unwrap();
        }

        out
    }

    // Both the `.cdt` binary payloads and their corresponding `.right` text fixtures
    // originate from the upstream libcdio GitHub repository reference samples.
    // Note: The text targets have been sanitized to align with our dump format by
    // converting indentation spaces to standard tabs (`\t`) and adding a trailing newline.
    const SAMPLES: [(&[u8], &str); 3] = [
        (
            include_bytes!("../../../fixtures/cdtext.cdt"),
            include_str!("../../../fixtures/cdtext.right"),
        ),
        (
            include_bytes!("../../../fixtures/cdtext-libburnia.cdt"),
            include_str!("../../../fixtures/cdtext-libburnia.right"),
        ),
        (
            include_bytes!("../../../fixtures/cdtext-krosis.cdt"),
            include_str!("../../../fixtures/cdtext-krosis.right"),
        ),
    ];

    #[test]
    fn t_libcdio_samples() {
        for (left, right) in SAMPLES {
            let metadata = Metadata::parse_packs(left).unwrap().unwrap();
            let dump = dump(&metadata).to_owned();
            assert_eq!(dump, right)
        }
    }
}
