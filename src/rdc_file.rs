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
        })
    }

    /// Look up a section by its `SectionType` enum value.
    pub fn section(&self, section_type: u32) -> Option<&SectionInfo> {
        self.sections
            .iter()
            .find(|section| section.section_type == section_type)
    }
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
    fn rejects_bad_magic_and_truncated_files() {
        assert!(RdcFile::parse(b"NOPE").is_err());
        assert!(RdcFile::parse(&[]).is_err());
        assert!(RdcFile::parse(&MAGIC.to_le_bytes()).is_err());
    }
}
