/*!
# Rip Rip Hooray: CD-Text Parser
*/

use std::collections::HashMap;

use crate::CDTextKind;

/// Enumeration of possible CD-Text languages.
///
/// The language code is encoded as specified in ANNEX 1 to part 5 of EBU
/// Tech 32 58 -E (1991).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Language {
    #[default]
    Unknown = 0x00,
    Albanian = 0x01,
    Breton = 0x02,
    Catalan = 0x03,
    Croatian = 0x04,
    Welsh = 0x05,
    Czech = 0x06,
    Danish = 0x07,
    German = 0x08,
    English = 0x09,
    Spanish = 0x0A,
    Esperanto = 0x0B,
    Estonian = 0x0C,
    Basque = 0x0D,
    Faroese = 0x0E,
    French = 0x0F,
    Frisian = 0x10,
    Irish = 0x11,
    Gaelic = 0x12,
    Galician = 0x13,
    Icelandic = 0x14,
    Italian = 0x15,
    Lappish = 0x16,
    Latin = 0x17,
    Latvian = 0x18,
    Luxembourgian = 0x19,
    Lithuanian = 0x1A,
    Hungarian = 0x1B,
    Maltese = 0x1C,
    Dutch = 0x1D,
    Norwegian = 0x1E,
    Occitan = 0x1F,
    Polish = 0x20,
    Portuguese = 0x21,
    Romanian = 0x22,
    Romansh = 0x23,
    Serbian = 0x24,
    Slovak = 0x25,
    Slovenian = 0x26,
    Finnish = 0x27,
    Swedish = 0x28,
    Turkish = 0x29,
    Flemish = 0x2A,
    Wallon = 0x2B,
    // 0x2C through 0x44 are unassigned gaps in the EBU specification.
    Zulu = 0x45,
    Vietnamese = 0x46,
    Uzbek = 0x47,
    Urdu = 0x48,
    Ukrainian = 0x49,
    Thai = 0x4A,
    Telugu = 0x4B,
    Tatar = 0x4C,
    Tamil = 0x4D,
    Tadzhik = 0x4E,
    Swahili = 0x4F,
    Sranantongo = 0x50,
    Somali = 0x51,
    Sinhalese = 0x52,
    Shona = 0x53,
    SerboCroat = 0x54,
    // 0x55 is unassigned.
    Russian = 0x56,
    Quechua = 0x57,
    Pushtu = 0x58,
    Punjabi = 0x59,
    Persian = 0x5A,
    Papamiento = 0x5B,
    Oriya = 0x5C,
    Nepali = 0x5D,
    Ndebele = 0x5E,
    Marathi = 0x5F,
    Moldavian = 0x60,
    Malaysian = 0x61,
    Malagasay = 0x62,
    Macedonian = 0x63,
    Laotian = 0x64,
    Korean = 0x65,
    Khmer = 0x66,
    Kazakh = 0x67,
    Kannada = 0x68,
    Japanese = 0x69,
    Indonesian = 0x6A,
    Hindi = 0x6B,
    Hebrew = 0x6C,
    Hausa = 0x6D,
    Gurani = 0x6E,
    Gujurati = 0x6F,
    Greek = 0x70,
    Georgian = 0x71,
    Fulani = 0x72,
    Dari = 0x73,
    Churash = 0x74,
    Chinese = 0x75,
    Burmese = 0x76,
    Bulgarian = 0x77,
    Bengali = 0x78,
    Bielorussian = 0x79,
    Bambora = 0x7A,
    Azerbaijani = 0x7B,
    Assamese = 0x7C,
    Armenian = 0x7D,
    Arabic = 0x7E,
    Amharic = 0x7F,
}

impl TryFrom<u8> for Language {
    type Error = u8;

    #[expect(unsafe_code, reason = "For FFI.")]
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00..=0x2B | 0x45..=0x54 | 0x56..=0x7F => unsafe { Ok(std::mem::transmute(value)) },
            unmapped => Err(unmapped),
        }
    }
}

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
    TocInfo = 0x88,
    TocInfo2 = 0x89,
    UpcEan = 0x8E,
    SizeInfo = 0x8F,
}

impl Field {
    pub(super) const ISRC: Field = Field::UpcEan;
}

impl TryFrom<u8> for Field {
    type Error = u8;

    #[expect(unsafe_code, reason = "For FFI.")]
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x80..=0x89 | 0x8E..=0x8F => unsafe { Ok(std::mem::transmute(value)) },
            unmapped => Err(unmapped),
        }
    }
}

impl Field {
    fn is_data(&self) -> bool {
        matches!(self, Field::TocInfo | Field::TocInfo2 | Field::SizeInfo)
    }

    fn is_text(&self) -> bool {
        !self.is_data()
    }
}

impl From<CDTextKind> for Field {
    fn from(value: CDTextKind) -> Self {
        match value {
            CDTextKind::Arranger => Self::Arranger,
            CDTextKind::Barcode => Self::UpcEan,
            CDTextKind::Composer => Self::Composer,
            CDTextKind::Isrc => Self::ISRC,
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
    fn decode(&self, bytes: &[u8]) -> String {
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

    #[cfg(test)]
    fn genre_code(&self) -> Option<GenreCode> {
        let genre_str = self.catalog.get(&(Field::Genre, 0))?;
        let bytes = genre_str.as_bytes();

        // The very first byte is our binary code.
        let raw_code = *bytes.first()?;

        GenreCode::try_from(raw_code).ok()
    }
}

#[derive(Debug)]
pub(super) enum Error {
    InvalidPack,
    InvalidEncoding,
    MissingSizeInfo,
    InvalidPayloadLength,
    InvalidPackCount,
    UnsupportedExtension,
    UnsupportedDoubleByte,
}

#[derive(Debug, Default)]
pub(super) struct Metadata {
    pub layers: Vec<LanguageLayer>,
}

#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
struct SizeInfo {
    pub char_code: u8,
    pub first_track: u8,
    pub last_track: u8,
    pub copyright: u8, // 3: CD-Text is copyrighted, 0: no copyright on CD-Text
    pub pack_counts: [u8; 16], // 16 pack types (0x80 through 0x8F)
    pub last_seq: [u8; 8], // Last sequence number for blocks 0..7
    pub lang_code: [u8; 8], // Language code for blocks 0..7
}

impl TryFrom<&[u8]> for SizeInfo {
    type Error = Error;

    fn try_from(buf: &[u8]) -> Result<Self, Self::Error> {
        if buf.len() != 36 {
            return Err(Error::InvalidPayloadLength);
        }

        let mut pack_counts = [0u8; 16];
        pack_counts.copy_from_slice(&buf[4..20]);

        let mut last_seq = [0u8; 8];
        last_seq.copy_from_slice(&buf[20..28]);

        let mut lang_code = [0u8; 8];
        lang_code.copy_from_slice(&buf[28..36]);

        Ok(Self {
            char_code: buf[0],
            first_track: buf[1],
            last_track: buf[2],
            copyright: buf[3],
            pack_counts,
            last_seq,
            lang_code,
        })
    }
}

impl SizeInfo {
    fn total_expected_packs(&self) -> usize {
        self.pack_counts.iter().map(|&count| count as usize).sum()
    }
}

impl Metadata {
    pub(super) fn from_bytes(buf: &[u8]) -> Result<Option<Self>, Error> {
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
        const PACK_CRC_OFFSET: usize = PACK_HEADER_LEN + PACK_PAYLOAD_LEN;

        type Pack = [u8; PACK_LEN];

        trait PackExt {
            /// Validates a raw 18-byte CD-Text pack using its trailing 2-byte CRC.
            fn is_valid(&self) -> bool;
        }

        impl PackExt for Pack {
            fn is_valid(&self) -> bool {
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
                let crc = u16::from_be_bytes([self[PACK_CRC_OFFSET], self[PACK_CRC_OFFSET + 1]]);

                ENGINE.checksum(&self[0..PACK_CRC_OFFSET]) == crc
            }
        }

        #[derive(Debug, Clone, Default)]
        struct Block {
            pack_count: usize,
            buffer: HashMap<(Field, u8), Vec<u8>>,
        }

        #[derive(Debug, Default)]
        struct Context {
            text_buf: Vec<u8>,
            language_blocks: Vec<Block>,
        }

        impl Context {
            pub(super) fn parse_pack(&mut self, pack: &Pack) -> Result<(), Error> {
                let header = &pack[0..PACK_HEADER_LEN];
                let payload = &pack[PACK_HEADER_LEN..PACK_CRC_OFFSET];

                let (id1, id2, id3, id4) = (header[0], header[1], header[2], header[3]);

                let is_extension = (id2 & 0x80) != 0; // Extension Flag (0 = normal, 1 = extension)
                if is_extension {
                    return Err(Error::UnsupportedExtension);
                }
                let mut track_number = id2 & 0x7F;
                let _sequence_number = id3;
                let block_id = (id4 >> 4) & 0x07; // Bits 4-6 define the language block ID.

                self.language_blocks
                    .resize(block_id as usize + 1, Block::default());

                self.language_blocks[block_id as usize].pack_count += 1;

                let Ok(field) = Field::try_from(id1) else {
                    // Safe early exit per CD-Text specification guidelines.
                    return Ok(());
                };

                if field.is_text() {
                    let _char_pos = id4 & 0x0f;
                    let is_double_byte = (id4 & 0x80) != 0;
                    if is_double_byte {
                        return Err(Error::UnsupportedDoubleByte);
                    }
                    for b in payload {
                        if *b == 0x00 {
                            if !self.text_buf.is_empty() {
                                let key = (field, track_number);
                                self.language_blocks[block_id as usize]
                                    .buffer
                                    .insert(key, self.text_buf.clone());
                                self.text_buf.clear();
                            }
                            track_number += 1;
                        } else if *b == b'\t' {
                            // Handle repetition.
                            let last_key = (field, track_number.saturating_sub(1));
                            let cloned_buf = self.language_blocks[block_id as usize]
                                .buffer
                                .get(&last_key)
                                .cloned();
                            if let Some(buf) = cloned_buf {
                                let key = (field, track_number);
                                self.language_blocks[block_id as usize]
                                    .buffer
                                    .insert(key, buf);
                            }
                        } else {
                            self.text_buf.push(*b);
                        }
                    }
                } else {
                    let key = (field, 0);
                    let buffer = self.language_blocks[block_id as usize]
                        .buffer
                        .entry(key)
                        .or_insert(Default::default());
                    buffer.extend_from_slice(payload);
                }
                Ok(())
            }
        }

        let mut context = Context::default();
        let (chunks, _remainder) = pack_data.as_chunks::<PACK_LEN>();
        for pack in chunks {
            if !pack.is_valid() {
                return Err(Error::InvalidPack);
            }
            context.parse_pack(pack)?;
        }

        let mut metadata = Self::default();
        for (i, block) in context.language_blocks.into_iter().enumerate() {
            let slice = block
                .buffer
                .get(&(Field::SizeInfo, 0))
                .ok_or(Error::MissingSizeInfo)?
                .as_slice();
            let size_info = SizeInfo::try_from(slice)?;

            if block.pack_count != size_info.total_expected_packs() {
                return Err(Error::InvalidPackCount);
            }
            let lang_code = size_info.lang_code[i];
            let char_code = size_info.char_code;
            let language = Language::try_from(lang_code).map_err(|_| Error::InvalidEncoding)?;
            let encoding = Encoding::try_from(char_code).map_err(|_| Error::InvalidEncoding)?;

            let mut layer = LanguageLayer::default();
            layer.first_track = size_info.first_track;
            layer.last_track = size_info.last_track;
            layer.language = language;
            for ((field, track), buf) in block.buffer {
                match field {
                    _ if field.is_text() => {
                        layer.catalog.insert((field, track), encoding.decode(&buf));
                    }
                    Field::DiscId | Field::UpcEan => {
                        if let Ok(s) = String::from_utf8(buf) {
                            layer.catalog.insert((field, track), s);
                        }
                    }
                    _ => (),
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
                dump_field(&mut out, "ISRC", layer, Field::ISRC, track);
            }

            writeln!(&mut out).unwrap();
        }

        out
    }

    macro_rules! samples_dir {
        () => {
            "../../../skel/cdtext/"
        };
    }

    // Both the `.cdt` binary payloads and their corresponding `.right` text fixtures
    // originate from the upstream libcdio GitHub repository reference samples.
    // Note: The text targets have been sanitized to align with our dump format by
    // converting indentation spaces to standard tabs (`\t`) and adding a trailing newline.
    const SAMPLES: [(&[u8], &str); 5] = [
        (
            include_bytes!(concat!(samples_dir!(), "cdtext.cdt")),
            include_str!(concat!(samples_dir!(), "cdtext.right")),
        ),
        (
            include_bytes!(concat!(samples_dir!(), "cdtext-libburnia.cdt")),
            include_str!(concat!(samples_dir!(), "cdtext-libburnia.right")),
        ),
        (
            include_bytes!(concat!(samples_dir!(), "cdtext-krosis.cdt")),
            include_str!(concat!(samples_dir!(), "cdtext-krosis.right")),
        ),
        (
            include_bytes!(concat!(samples_dir!(), "simple.cdt")),
            include_str!(concat!(samples_dir!(), "simple.right")),
        ),
        (
            include_bytes!(concat!(samples_dir!(), "double.cdt")),
            include_str!(concat!(samples_dir!(), "double.right")),
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
