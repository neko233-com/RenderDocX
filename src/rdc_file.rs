//! Read-only `.rdc` container parser, translated from the authoritative format
//! documented in `renderdoc/serialise/rdcfile.cpp` (lines 46-140).
//!
//! This parses the file container only: header, thumbnail metadata, capture
//! metadata and the section table. Decoding individual section *contents*
//! (the binary structured-data format) is a separate, larger layer that still
//! lives in the replay core. A native container reader is the first step to
//! removing the C++ bridge: it lets the GUI inspect a capture's structure
//! (sections, types, versions, compression) with zero C++ and zero GPU.
//!
//! All byte offsets/endianness mirror the upstream struct layout (little-endian,
//! tightly packed/unaligned).

/// `FileType` enum values from `renderdoc/api/replay/replay_enums.h`
/// (image formats used by the ExtendedThumbnail section).
pub mod file_type {
    pub const DDS: u32 = 0;
    pub const PNG: u32 = 1;
    pub const JPG: u32 = 2;
    pub const BMP: u32 = 3;
    pub const TGA: u32 = 4;
    pub const HDR: u32 = 5;
    pub const EXR: u32 = 6;
    pub const RAW: u32 = 7;

    pub fn name(file_type: u32) -> &'static str {
        match file_type {
            DDS => "DDS",
            PNG => "PNG",
            JPG => "JPG",
            BMP => "BMP",
            TGA => "TGA",
            HDR => "HDR",
            EXR => "EXR",
            RAW => "Raw",
            _ => "Unknown",
        }
    }
}

/// Decoded extended thumbnail image (RGBA8 pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct ExtendedThumbnail {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub rgba: Vec<u8>,
}

/// `SectionType` enum values from `renderdoc/api/replay/replay_enums.h`.
pub mod section_type {
    pub const UNKNOWN: u32 = 0;
    pub const FRAME_CAPTURE: u32 = 1;
    pub const RESOLVE_DATABASE: u32 = 2;
    pub const BOOKMARKS: u32 = 3;
    pub const NOTES: u32 = 4;
    pub const RESOURCE_RENAMES: u32 = 5;
    pub const AMD_RGP_PROFILE: u32 = 6;
    pub const EXTENDED_THUMBNAIL: u32 = 7;
    pub const EMBEDDED_LOGFILE: u32 = 8;
    pub const EDITED_SHADERS: u32 = 9;
    pub const D3D12_CORE: u32 = 10;
    pub const D3D12_SDK_LAYERS: u32 = 11;
    pub const EMBEDDED_EXTERNAL_FILES: u32 = 12;
}

/// Human-readable section type name (upstream `SectionType` enum names).
pub fn section_type_name(section_type: u32) -> &'static str {
    use section_type::*;
    match section_type {
        UNKNOWN => "Unknown",
        FRAME_CAPTURE => "FrameCapture",
        RESOLVE_DATABASE => "ResolveDatabase",
        BOOKMARKS => "Bookmarks",
        NOTES => "Notes",
        RESOURCE_RENAMES => "ResourceRenames",
        AMD_RGP_PROFILE => "AMDRGPProfile",
        EXTENDED_THUMBNAIL => "ExtendedThumbnail",
        EMBEDDED_LOGFILE => "EmbeddedLogfile",
        EDITED_SHADERS => "EditedShaders",
        D3D12_CORE => "D3D12Core",
        D3D12_SDK_LAYERS => "D3D12SDKLayers",
        EMBEDDED_EXTERNAL_FILES => "EmbeddedExternalFiles",
        _ => "Invalid",
    }
}

/// `MAKE_FOURCC('R','D','O','C')` = 0x434F4452, little-endian bytes "RDOC".
pub const MAGIC: u32 = 0x434f_4452;

pub const FLAG_ASCII: u32 = 0x1;
pub const FLAG_LZ4: u32 = 0x2;
pub const FLAG_ZSTD: u32 = 0x4;

#[derive(Clone, Debug, PartialEq)]
pub struct RdcHeader {
    pub version: u32,
    pub header_length: u32,
    pub program_version: String,
    pub thumb_width: u16,
    pub thumb_height: u16,
    pub thumb_length: u32,
    pub machine_ident: u64,
    pub driver_id: u32,
    pub driver_name: String,
    pub time_base: Option<u64>,
    pub time_freq: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionInfo {
    pub section_type: u32,
    pub compressed_length: u64,
    pub uncompressed_length: u64,
    pub version: u64,
    pub flags: u32,
    pub name: String,
    /// Byte offset of the section's data payload within the file.
    pub data_offset: usize,
}

#[derive(Clone, Debug)]
pub struct RdcFile {
    pub header: RdcHeader,
    pub sections: Vec<SectionInfo>,
    /// Raw file bytes, retained so section payloads can be decoded on demand.
    data: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(count).ok_or("read overflow")?;
        if end > self.data.len() {
            return Err("unexpected end of .rdc file".to_owned());
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn cstring(&mut self, len: usize) -> Result<String, String> {
        let bytes = self.take(len)?;
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
}

impl RdcFile {
    /// Parse a `.rdc` file from raw bytes. Returns a structured description of
    /// the container; section *contents* are not decoded.
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        let mut r = Reader { data, pos: 0 };
        if r.u32()? != MAGIC {
            return Err("not a RenderDoc .rdc file (bad magic)".to_owned());
        }
        let version = r.u32()?;
        let header_length = r.u32()?;
        let program_version = r.cstring(16)?;
        let thumb_width = r.u16()?;
        let thumb_height = r.u16()?;
        let thumb_length = r.u32()?;
        let _thumbnail = r.take(thumb_length as usize)?;

        let machine_ident = r.u64()?;
        let driver_id = r.u32()?;
        let driver_name_len = r.take(1)?[0] as usize;
        let driver_name = r.cstring(driver_name_len)?;

        let (time_base, time_freq) = if version >= 0x102 {
            (Some(r.u64()?), Some(r.f64()?))
        } else {
            (None, None)
        };

        // Sections begin at headerLength from the start of the file.
        if (header_length as usize) < r.pos {
            return Err("headerLength is smaller than the parsed header".to_owned());
        }
        r.pos = header_length as usize;

        let mut sections = Vec::new();
        while r.pos < data.len() {
            let marker = r.take(1)?[0];
            if marker == b'A' {
                // ASCII section: \n length \n type \n version \n name \n payload
                if r.take(1)?[0] != b'\n' {
                    return Err("malformed ASCII section header".to_owned());
                }
                let len = read_ascii_line(&mut r)?
                    .parse::<u64>()
                    .map_err(|_| "bad ASCII length")?;
                let section_type = read_ascii_line(&mut r)?
                    .parse::<u32>()
                    .map_err(|_| "bad ASCII type")?;
                let section_version = read_ascii_line(&mut r)?
                    .parse::<u64>()
                    .map_err(|_| "bad ASCII version")?;
                let name = read_ascii_line(&mut r)?;
                let data_offset = r.pos;
                r.take(len as usize)?;
                sections.push(SectionInfo {
                    section_type,
                    compressed_length: len,
                    uncompressed_length: len,
                    version: section_version,
                    flags: FLAG_ASCII,
                    name,
                    data_offset,
                });
            } else {
                // Binary section: 3 pad bytes follow the marker.
                r.take(3)?;
                let section_type = r.u32()?;
                let compressed_length = r.u64()?;
                let uncompressed_length = r.u64()?;
                let version = r.u64()?;
                let flags = r.u32()?;
                let name_len = r.u32()? as usize;
                let name = r.cstring(name_len)?;
                let data_offset = r.pos;
                r.take(compressed_length as usize)?;
                sections.push(SectionInfo {
                    section_type,
                    compressed_length,
                    uncompressed_length,
                    version,
                    flags,
                    name,
                    data_offset,
                });
            }
        }

        Ok(Self {
            header: RdcHeader {
                version,
                header_length,
                program_version,
                thumb_width,
                thumb_height,
                thumb_length,
                machine_ident,
                driver_id,
                driver_name,
                time_base,
                time_freq,
            },
            sections,
            data: data.to_vec(),
        })
    }

    /// Look up a section by its `SectionType` enum value.
    pub fn section(&self, section_type: u32) -> Option<&SectionInfo> {
        self.sections
            .iter()
            .find(|section| section.section_type == section_type)
    }

    /// Extract the embedded AMD RGP profile, mirroring qrenderdoc's
    /// "Open RGP Profile" (`MainWindow.cpp:2877-2903`): locate the
    /// `AMDRGPProfile` section, decode its payload and return the raw `.rgp`
    /// bytes. Opening the profile still needs AMD's external RGP tooling, but
    /// extraction itself is pure Rust with no C++ bridge.
    pub fn extract_rgp_profile(&self) -> Result<Vec<u8>, String> {
        let section = self
            .section(section_type::AMD_RGP_PROFILE)
            .ok_or("Capture has no AMDRGPProfile section")?;
        self.section_payload(section)
    }

    /// Decode an ExtendedThumbnail section payload: `ExtThumbnailHeader`
    /// (u16 width, u16 height, u32 len, u32 format) followed by `len` image
    /// bytes (translated from `rdcfile.cpp:714`). PNG payloads are decoded to
    /// RGBA8 natively via the `png` crate; other formats return their bytes
    /// unparsed with the declared format.
    pub fn extended_thumbnail(&self, section: &SectionInfo) -> Result<ExtendedThumbnail, String> {
        if section.section_type != 7 {
            return Err("section is not ExtendedThumbnail".to_owned());
        }
        let payload = self.section_payload(section)?;
        if payload.len() < 12 {
            return Err("truncated ExtThumbnailHeader".to_owned());
        }
        let width = u16::from_le_bytes(payload[0..2].try_into().unwrap()) as u32;
        let height = u16::from_le_bytes(payload[2..4].try_into().unwrap()) as u32;
        let len = u32::from_le_bytes(payload[4..8].try_into().unwrap()) as usize;
        let format = u32::from_le_bytes(payload[8..12].try_into().unwrap());
        if 12 + len > payload.len() {
            return Err("thumbnail data exceeds section".to_owned());
        }
        let image = &payload[12..12 + len];
        let rgba = if format == file_type::PNG {
            decode_png_rgba(image)?
        } else {
            Vec::new()
        };
        Ok(ExtendedThumbnail {
            width,
            height,
            format,
            rgba,
        })
    }

    /// Read and decode a section's payload. Plain sections return their bytes
    /// directly; Zstd-compressed sections are decompressed with the mature
    /// `zstd` crate. LZ4-chunked sections are decoded with `lz4_flex`'s block
    /// decoder, translating the custom framing from `lz4io.cpp`.
    pub fn section_payload(&self, section: &SectionInfo) -> Result<Vec<u8>, String> {
        let start = section.data_offset;
        let end = start
            .checked_add(section.compressed_length as usize)
            .ok_or("section length overflow")?;
        if end > self.data.len() {
            return Err("section payload exceeds file".to_owned());
        }
        let raw = &self.data[start..end];
        let decoded = if section.flags & FLAG_ZSTD != 0 {
            zstd::stream::decode_all(raw)
                .map_err(|error| format!("Zstd decompression failed: {error}"))?
        } else if section.flags & FLAG_LZ4 != 0 {
            decompress_lz4_chunks(raw, section.uncompressed_length as usize)?
        } else {
            raw.to_vec()
        };
        if decoded.len() != section.uncompressed_length as usize {
            return Err(format!(
                "decompressed section length {} != expected {}",
                decoded.len(),
                section.uncompressed_length
            ));
        }
        Ok(decoded)
    }
}

/// Decode a PNG byte slice into RGBA8 pixels using the mature `png` crate.
fn decode_png_rgba(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("PNG decode failed: {error}"))?;
    let buffer_size = reader
        .output_buffer_size()
        .ok_or("PNG output buffer size unavailable")?;
    let mut output = vec![0u8; buffer_size];
    let info = reader
        .next_frame(&mut output)
        .map_err(|error| format!("PNG frame decode failed: {error}"))?;
    let pixels = &output[..info.buffer_size()];
    // Convert to RGBA8 if the source used a different color type.
    let color = info.color_type;
    let bytes_per_pixel = color.samples();
    if color == png::ColorType::Rgba {
        Ok(pixels.to_vec())
    } else {
        let mut rgba_out = Vec::with_capacity(info.width as usize * info.height as usize * 4);
        for pixel in pixels.chunks(bytes_per_pixel) {
            let (r, g, b, a) = match color {
                png::ColorType::Rgb => (pixel[0], pixel[1], pixel[2], 255),
                png::ColorType::Grayscale => (pixel[0], pixel[0], pixel[0], 255),
                png::ColorType::GrayscaleAlpha => (pixel[0], pixel[0], pixel[0], pixel[1]),
                _ => (0, 0, 0, 255),
            };
            rgba_out.extend_from_slice(&[r, g, b, a]);
        }
        Ok(rgba_out)
    }
}

/// Decompress RenderDoc's LZ4 section framing (translated from `lz4io.cpp`):
/// a stream of chunks, each prefixed with an `int32` compressed size, followed
/// by that many bytes of LZ4 block data. Each block is decoded with the
/// previous block as an external dictionary, mirroring the 64 KB history
/// window that `LZ4_decompress_safe_continue` maintains upstream.
fn decompress_lz4_chunks(raw: &[u8], expected: usize) -> Result<Vec<u8>, String> {
    const LZ4_BLOCK_SIZE: usize = 1024 * 1024;
    const MAX_COMPRESSED: usize = LZ4_BLOCK_SIZE + LZ4_BLOCK_SIZE / 255 + 16;
    let mut cursor = 0usize;
    let mut output: Vec<u8> = Vec::with_capacity(expected);
    let mut dictionary: Vec<u8> = Vec::new();
    while cursor < raw.len() {
        if cursor + 4 > raw.len() {
            return Err("truncated LZ4 chunk size".to_owned());
        }
        let size = i32::from_le_bytes(raw[cursor..cursor + 4].try_into().unwrap());
        cursor += 4;
        if size < 0 || size as usize > MAX_COMPRESSED {
            return Err(format!("invalid LZ4 compressed block size: {size}"));
        }
        let size = size as usize;
        if cursor + size > raw.len() {
            return Err("truncated LZ4 chunk data".to_owned());
        }
        let block = &raw[cursor..cursor + size];
        cursor += size;
        let mut buffer = vec![0u8; LZ4_BLOCK_SIZE];
        let written = lz4_flex::block::decompress_into_with_dict(block, &mut buffer, &dictionary)
            .map_err(|error| format!("LZ4 decompression failed: {error}"))?;
        if written > expected.saturating_sub(output.len()) {
            return Err("LZ4 output exceeds expected section size".to_owned());
        }
        // Keep the tail as the dictionary for the next block (64 KB history).
        dictionary = buffer[..written].to_vec();
        if dictionary.len() > 64 * 1024 {
            let start = dictionary.len() - 64 * 1024;
            dictionary.drain(..start);
        }
        output.extend_from_slice(&buffer[..written]);
    }
    if output.len() != expected {
        return Err(format!(
            "LZ4 decompressed length {} != expected {}",
            output.len(),
            expected
        ));
    }
    Ok(output)
}

fn read_ascii_line(r: &mut Reader<'_>) -> Result<String, String> {
    let start = r.pos;
    while r.pos < r.data.len() && r.data[r.pos] != b'\n' {
        r.pos += 1;
    }
    if r.pos >= r.data.len() {
        return Err("unterminated ASCII section line".to_owned());
    }
    let line = String::from_utf8_lossy(&r.data[start..r.pos]).into_owned();
    r.pos += 1;
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn build_rdc(version: u32, sections: &[SectionInfo], payloads: &[&[u8]]) -> Vec<u8> {
        let mut buf = Vec::new();
        // Fixed header portion; headerLength patched after we know it.
        buf.extend_from_slice(&MAGIC.to_le_bytes());
        buf.extend_from_slice(&version.to_le_bytes());
        let header_len_offset = buf.len();
        buf.extend_from_slice(&0u32.to_le_bytes()); // placeholder headerLength
        let mut prog = [0u8; 16];
        prog[..5].copy_from_slice(b"v1.48");
        buf.extend_from_slice(&prog);
        buf.extend_from_slice(&0u16.to_le_bytes()); // thumb width
        buf.extend_from_slice(&0u16.to_le_bytes()); // thumb height
        buf.extend_from_slice(&0u32.to_le_bytes()); // thumb length
        buf.extend_from_slice(&7u64.to_le_bytes()); // machine ident
        buf.extend_from_slice(&3u32.to_le_bytes()); // driver id
        buf.push(6); // driver name length incl NUL
        buf.extend_from_slice(b"D3D11\0");
        if version >= 0x102 {
            buf.extend_from_slice(&100u64.to_le_bytes()); // time base
            buf.extend_from_slice(&1_000_000.0f64.to_le_bytes()); // time freq
        }
        let header_len = buf.len() as u32;
        buf[header_len_offset..header_len_offset + 4].copy_from_slice(&header_len.to_le_bytes());
        for (section, payload) in sections.iter().zip(payloads) {
            buf.write_all(&[0]).unwrap(); // ASCII marker = 0 for binary
            buf.write_all(&[0, 0, 0]).unwrap(); // pad
            buf.write_all(&section.section_type.to_le_bytes()).unwrap();
            buf.write_all(&section.compressed_length.to_le_bytes())
                .unwrap();
            buf.write_all(&section.uncompressed_length.to_le_bytes())
                .unwrap();
            buf.write_all(&section.version.to_le_bytes()).unwrap();
            buf.write_all(&section.flags.to_le_bytes()).unwrap();
            buf.write_all(&((section.name.len() + 1) as u32).to_le_bytes())
                .unwrap();
            buf.write_all(section.name.as_bytes()).unwrap();
            buf.push(0);
            buf.write_all(payload).unwrap();
        }
        buf
    }

    #[test]
    fn parses_binary_sections() {
        let sections = vec![
            SectionInfo {
                section_type: 1,
                compressed_length: 4,
                uncompressed_length: 4,
                version: 2,
                flags: 0,
                name: "frame".to_owned(),
                data_offset: 0,
            },
            SectionInfo {
                section_type: 9,
                compressed_length: 3,
                uncompressed_length: 8,
                version: 0,
                flags: FLAG_ZSTD,
                name: "notes".to_owned(),
                data_offset: 0,
            },
        ];
        let bytes = build_rdc(0x102, &sections, &[&[1, 2, 3, 4], &[9, 9, 9]]);
        let parsed = RdcFile::parse(&bytes).unwrap();
        assert_eq!(parsed.header.program_version, "v1.48");
        assert_eq!(parsed.header.driver_name, "D3D11");
        assert_eq!(parsed.header.version, 0x102);
        assert_eq!(parsed.header.time_base, Some(100));
        assert_eq!(parsed.sections.len(), 2);
        let frame = parsed.section(1).unwrap();
        assert_eq!(frame.name, "frame");
        assert_eq!(frame.version, 2);
        assert_eq!(frame.flags, 0);
        let notes = parsed.section(9).unwrap();
        assert_eq!(notes.flags, FLAG_ZSTD);
        assert_eq!(notes.uncompressed_length, 8);
    }

    #[test]
    fn parses_ascii_section() {
        let mut buf = Vec::new();
        // version 0x100 header (no time fields)
        let bytes = build_rdc(0x100, &[], &[]);
        buf.extend_from_slice(&bytes);
        // append an ASCII section: 'A' \n len \n type \n version \n name \n payload
        let payload = b"hello";
        buf.extend_from_slice(b"A\n");
        buf.extend_from_slice(format!("{}\n", payload.len()).as_bytes());
        buf.extend_from_slice(b"7\n0\nextra\n");
        buf.extend_from_slice(payload);
        let parsed = RdcFile::parse(&buf).unwrap();
        assert_eq!(parsed.header.time_base, None);
        assert_eq!(parsed.sections.len(), 1);
        let section = &parsed.sections[0];
        assert_eq!(section.section_type, 7);
        assert_eq!(section.name, "extra");
        assert_eq!(section.flags, FLAG_ASCII);
        assert_eq!(section.compressed_length, 5);
    }

    #[test]
    fn decodes_plain_and_zstd_section_payloads() {
        let plain_payload: &[u8] = b"plain section contents";
        let compressed_source: &[u8] = b"this will be zstd compressed";
        let zstd_payload = zstd::stream::encode_all(compressed_source, 0).unwrap();

        let sections = vec![
            SectionInfo {
                section_type: 1,
                compressed_length: plain_payload.len() as u64,
                uncompressed_length: plain_payload.len() as u64,
                version: 0,
                flags: 0,
                name: "plain".to_owned(),
                data_offset: 0,
            },
            SectionInfo {
                section_type: 2,
                compressed_length: zstd_payload.len() as u64,
                uncompressed_length: compressed_source.len() as u64,
                version: 0,
                flags: FLAG_ZSTD,
                name: "compressed".to_owned(),
                data_offset: 0,
            },
        ];
        let bytes = build_rdc(0x102, &sections, &[plain_payload, &zstd_payload]);
        let parsed = RdcFile::parse(&bytes).unwrap();

        let plain = parsed.section(1).unwrap();
        assert_eq!(parsed.section_payload(plain).unwrap(), plain_payload);
        let compressed = parsed.section(2).unwrap();
        assert_eq!(
            parsed.section_payload(compressed).unwrap(),
            compressed_source
        );
    }

    #[test]
    fn decodes_lz4_chunked_section() {
        let source: &[u8] = b"LZ4 chunked section payload, repeated repeated repeated";
        let mut scratch = vec![0u8; source.len() + 64];
        let written = lz4_flex::block::compress_into(source, &mut scratch).unwrap();
        let mut framed = Vec::new();
        framed.extend_from_slice(&(written as i32).to_le_bytes());
        framed.extend_from_slice(&scratch[..written]);

        let sections = vec![SectionInfo {
            section_type: 3,
            compressed_length: framed.len() as u64,
            uncompressed_length: source.len() as u64,
            version: 0,
            flags: FLAG_LZ4,
            name: "lz4".to_owned(),
            data_offset: 0,
        }];
        let bytes = build_rdc(0x102, &sections, &[&framed]);
        let parsed = RdcFile::parse(&bytes).unwrap();
        let section = parsed.section(3).unwrap();
        assert_eq!(parsed.section_payload(section).unwrap(), source);
    }

    #[test]
    fn decodes_extended_thumbnail_png() {
        // Build a 1x1 RGBA PNG with the mature png crate.
        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[255, 0, 0, 255]).unwrap();
        }
        let mut payload = Vec::new();
        payload.extend_from_slice(&1u16.to_le_bytes()); // width
        payload.extend_from_slice(&1u16.to_le_bytes()); // height
        payload.extend_from_slice(&(png_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(&file_type::PNG.to_le_bytes());
        payload.extend_from_slice(&png_bytes);

        let sections = vec![SectionInfo {
            section_type: 7,
            compressed_length: payload.len() as u64,
            uncompressed_length: payload.len() as u64,
            version: 0,
            flags: 0,
            name: "thumbnail".to_owned(),
            data_offset: 0,
        }];
        let bytes = build_rdc(0x102, &sections, &[&payload]);
        let parsed = RdcFile::parse(&bytes).unwrap();
        let thumb = parsed
            .extended_thumbnail(parsed.section(7).unwrap())
            .unwrap();
        assert_eq!(thumb.width, 1);
        assert_eq!(thumb.height, 1);
        assert_eq!(thumb.format, file_type::PNG);
        assert_eq!(thumb.rgba, vec![255, 0, 0, 255]);
    }

    #[test]
    fn extracts_embedded_rgp_profile() {
        let profile: &[u8] = b"RGP-proprietary-profile-bytes";
        let sections = vec![SectionInfo {
            section_type: section_type::AMD_RGP_PROFILE,
            compressed_length: profile.len() as u64,
            uncompressed_length: profile.len() as u64,
            version: 0,
            flags: 0,
            name: "AMD RGP Profile".to_owned(),
            data_offset: 0,
        }];
        let bytes = build_rdc(0x102, &sections, &[profile]);
        let parsed = RdcFile::parse(&bytes).unwrap();
        assert_eq!(parsed.extract_rgp_profile().unwrap(), profile);

        let empty = build_rdc(0x102, &[], &[]);
        let no_profile = RdcFile::parse(&empty).unwrap();
        assert!(no_profile.extract_rgp_profile().is_err());
    }

    #[test]
    fn section_type_names_match_upstream_enum() {
        assert_eq!(
            section_type_name(section_type::FRAME_CAPTURE),
            "FrameCapture"
        );
        assert_eq!(
            section_type_name(section_type::AMD_RGP_PROFILE),
            "AMDRGPProfile"
        );
        assert_eq!(
            section_type_name(section_type::EMBEDDED_EXTERNAL_FILES),
            "EmbeddedExternalFiles"
        );
        assert_eq!(section_type_name(section_type::UNKNOWN), "Unknown");
        assert_eq!(section_type_name(999), "Invalid");
    }

    #[test]
    fn rejects_bad_magic_and_truncated_files() {
        assert!(RdcFile::parse(b"NOPE").is_err());
        assert!(RdcFile::parse(&[]).is_err());
        assert!(RdcFile::parse(&MAGIC.to_le_bytes()).is_err());
    }
}
