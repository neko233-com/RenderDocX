//! Reader for RenderDoc's binary structured-data encoding, translated from
//! `renderdoc/serialise/serialiser.cpp` (`Reading::BeginChunk`) and `serialiser.h`
//! (`SerialiseValue` for strings). This is the next layer above the `.rdc`
//! container: FrameCapture section payloads are a stream of these chunks.
//!
//! Chunk header (`uint32 c`):
//!   - low 16 bits: chunk index (0 is reserved/control)
//!   - high bits: presence flags for optional metadata, then the chunk length
//!     (32-bit unless `Chunk64BitSize`).

pub mod chunk_flags {
    pub const CHUNK_INDEX_MASK: u32 = 0x0000_ffff;
    pub const CHUNK_CALLSTACK: u32 = 0x0001_0000;
    pub const CHUNK_THREAD_ID: u32 = 0x0002_0000;
    pub const CHUNK_DURATION: u32 = 0x0004_0000;
    pub const CHUNK_TIMESTAMP: u32 = 0x0008_0000;
    pub const CHUNK_64BIT_SIZE: u32 = 0x0010_0000;
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChunkHeader {
    pub chunk_id: u16,
    pub has_callstack: bool,
    pub thread_id: Option<u32>,
    pub duration_micro: Option<i64>,
    pub timestamp_micro: Option<i64>,
    pub callstack: Vec<u64>,
    /// Byte length of the chunk payload that follows the header.
    pub length: u64,
    /// Byte offset where the chunk payload begins.
    pub payload_offset: usize,
}

#[derive(Clone, Debug)]
pub struct BinaryCursor<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> BinaryCursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(count).ok_or("read overflow")?;
        if end > self.data.len() {
            return Err("unexpected end of structured data".to_owned());
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn i64(&mut self) -> Result<i64, String> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// RenderDoc `Serialiser` aligns byte buffers to 64-byte chunk alignment
    /// before reading (`m_Read->AlignTo<ChunkAlignment>()`, ChunkAlignment=64).
    pub fn align_to(&mut self, alignment: usize) {
        if alignment == 0 {
            return;
        }
        let remainder = self.pos % alignment;
        if remainder != 0 {
            self.pos += alignment - remainder;
        }
    }

    /// Read a raw byte buffer, applying upstream's 64-byte alignment first.
    pub fn byte_buffer(&mut self, count: usize) -> Result<&'a [u8], String> {
        self.align_to(64);
        self.take(count)
    }

    /// RenderDoc `rdcstr`: int32 length (-1 = null), then `len` raw bytes.
    pub fn string(&mut self) -> Result<Option<String>, String> {
        let len = self.i32()?;
        if len == -1 {
            return Ok(None);
        }
        if len < 0 {
            return Err("invalid string length".to_owned());
        }
        let bytes = self.take(len as usize)?;
        Ok(Some(String::from_utf8_lossy(bytes).into_owned()))
    }
}

/// Read one chunk header at the cursor's current position, advancing the
/// cursor to the start of the chunk payload.
pub fn read_chunk_header(cursor: &mut BinaryCursor<'_>) -> Result<ChunkHeader, String> {
    use chunk_flags::*;
    let c = cursor.u32()?;
    let chunk_id = (c & CHUNK_INDEX_MASK) as u16;
    let mut callstack = Vec::new();
    let has_callstack = c & CHUNK_CALLSTACK != 0;
    if has_callstack {
        let frames = cursor.u32()?;
        if frames < 4096 {
            for _ in 0..frames {
                callstack.push(cursor.u64()?);
            }
        } else {
            return Err("invalid callstack frame count".to_owned());
        }
    }
    let thread_id = if c & CHUNK_THREAD_ID != 0 {
        Some(cursor.u32()?)
    } else {
        None
    };
    let duration_micro = if c & CHUNK_DURATION != 0 {
        Some(cursor.i64()?)
    } else {
        None
    };
    let timestamp_micro = if c & CHUNK_TIMESTAMP != 0 {
        Some(cursor.i64()?)
    } else {
        None
    };
    let length = if c & CHUNK_64BIT_SIZE != 0 {
        cursor.u64()?
    } else {
        cursor.u32()? as u64
    };
    Ok(ChunkHeader {
        chunk_id,
        has_callstack,
        thread_id,
        duration_micro,
        timestamp_micro,
        callstack,
        length,
        payload_offset: cursor.pos,
    })
}

/// Walk the top-level chunk stream of a structured-data section. Returns the
/// headers; each payload spans `payload_offset..payload_offset+length`.
pub fn read_chunks(data: &[u8]) -> Result<Vec<ChunkHeader>, String> {
    let mut cursor = BinaryCursor::new(data);
    let mut chunks = Vec::new();
    while cursor.pos < data.len() {
        let header = read_chunk_header(&mut cursor)?;
        let end = cursor
            .pos
            .checked_add(header.length as usize)
            .ok_or("chunk length overflow")?;
        if end > data.len() {
            return Err("chunk payload exceeds section".to_owned());
        }
        cursor.pos = end;
        chunks.push(header);
    }
    Ok(chunks)
}

/// `SDBasic` category values from `structured_data.h` (enum class : uint32_t).
pub mod sdbasic {
    pub const CHUNK: u32 = 0;
    pub const STRUCT: u32 = 1;
    pub const ARRAY: u32 = 2;
    pub const NULL: u32 = 3;
    pub const BUFFER: u32 = 4;
    pub const STRING: u32 = 5;
    pub const ENUM: u32 = 6;
    pub const UNSIGNED: u32 = 7;
    pub const SIGNED: u32 = 8;
    pub const FLOAT: u32 = 9;
    pub const BOOLEAN: u32 = 10;
    pub const CHARACTER: u32 = 11;
    pub const RESOURCE: u32 = 12;
    pub const GPU_ADDRESS: u32 = 13;
}

#[derive(Clone, Debug, PartialEq)]
pub struct SdType {
    pub name: String,
    pub basetype: u32,
    pub flags: u32,
    pub byte_size: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SdObject {
    pub name: String,
    pub object_type: SdType,
    /// Raw basic value (u64 bits; interpret per `object_type.basetype`).
    pub basic: u64,
    pub string: Option<String>,
    pub children: Vec<SdObject>,
}

/// Read a self-describing `SDObject` tree in the wire format produced by
/// `DoSerialise(SDObject&)` in `serialiser.cpp` (lines 774-781): name, type,
/// data (basic + string), then a u64 child count followed by recursive children.
/// Guards against pathological child counts to avoid runaway allocation.
pub fn read_sd_object(cursor: &mut BinaryCursor<'_>, depth: usize) -> Result<SdObject, String> {
    const MAX_DEPTH: usize = 128;
    const MAX_CHILDREN: u64 = 1_000_000;
    if depth > MAX_DEPTH {
        return Err("SDObject nesting too deep".to_owned());
    }
    let name = cursor.string()?.unwrap_or_default();
    let type_name = cursor.string()?.unwrap_or_default();
    let basetype = cursor.u32()?;
    let flags = cursor.u32()?;
    let byte_size = cursor.u64()?;
    let object_type = SdType {
        name: type_name,
        basetype,
        flags,
        byte_size,
    };
    let basic = cursor.u64()?;
    let string = cursor.string()?;
    let child_count = cursor.u64()?;
    if child_count > MAX_CHILDREN {
        return Err("SDObject child count too large".to_owned());
    }
    let mut children = Vec::with_capacity(child_count.min(4096) as usize);
    for _ in 0..child_count {
        children.push(read_sd_object(cursor, depth + 1)?);
    }
    Ok(SdObject {
        name,
        object_type,
        basic,
        string,
        children,
    })
}

/// Interpret the raw `basic` bits of an SdObject per its basetype.
pub fn basic_as_u32(object: &SdObject) -> Option<u32> {
    match object.object_type.basetype {
        sdbasic::UNSIGNED | sdbasic::ENUM | sdbasic::RESOURCE | sdbasic::GPU_ADDRESS => {
            Some(object.basic as u32)
        }
        sdbasic::BOOLEAN | sdbasic::CHARACTER => Some((object.basic & 0xff) as u32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chunk_flags::*;

    fn write_string(buf: &mut Vec<u8>, value: Option<&str>) {
        match value {
            Some(text) => {
                buf.extend_from_slice(&(text.len() as i32).to_le_bytes());
                buf.extend_from_slice(text.as_bytes());
            }
            None => buf.extend_from_slice(&(-1i32).to_le_bytes()),
        }
    }

    #[test]
    fn reads_self_describing_sdobject_tree() {
        let mut buf = Vec::new();
        // root: name="root", type("MyStruct", Struct, flags 0, byteSize 0),
        // basic=0, string=null, 2 children
        write_string(&mut buf, Some("root"));
        write_string(&mut buf, Some("MyStruct"));
        buf.extend_from_slice(&sdbasic::STRUCT.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u64.to_le_bytes());
        buf.extend_from_slice(&0u64.to_le_bytes()); // basic
        write_string(&mut buf, None);
        buf.extend_from_slice(&2u64.to_le_bytes()); // child count
                                                    // child 0: uint value 7
        write_string(&mut buf, Some("count"));
        write_string(&mut buf, Some("uint32_t"));
        buf.extend_from_slice(&sdbasic::UNSIGNED.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&4u64.to_le_bytes());
        buf.extend_from_slice(&7u64.to_le_bytes());
        write_string(&mut buf, None);
        buf.extend_from_slice(&0u64.to_le_bytes());
        // child 1: string "hello"
        write_string(&mut buf, Some("label"));
        write_string(&mut buf, Some("string"));
        buf.extend_from_slice(&sdbasic::STRING.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&5u64.to_le_bytes());
        buf.extend_from_slice(&0u64.to_le_bytes());
        write_string(&mut buf, Some("hello"));
        buf.extend_from_slice(&0u64.to_le_bytes());

        let mut cursor = BinaryCursor::new(&buf);
        let root = read_sd_object(&mut cursor, 0).unwrap();
        assert_eq!(root.name, "root");
        assert_eq!(root.object_type.basetype, sdbasic::STRUCT);
        assert_eq!(root.children.len(), 2);
        assert_eq!(basic_as_u32(&root.children[0]), Some(7));
        assert_eq!(root.children[1].string.as_deref(), Some("hello"));
        assert_eq!(cursor.pos, buf.len());
    }

    #[test]
    fn rejects_huge_child_count() {
        let mut buf = Vec::new();
        write_string(&mut buf, Some("x"));
        write_string(&mut buf, Some("t"));
        buf.extend_from_slice(&sdbasic::STRUCT.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u64.to_le_bytes());
        buf.extend_from_slice(&0u64.to_le_bytes());
        write_string(&mut buf, None);
        buf.extend_from_slice(&u64::MAX.to_le_bytes());
        let mut cursor = BinaryCursor::new(&buf);
        assert!(read_sd_object(&mut cursor, 0).is_err());
    }

    #[test]
    fn reads_minimal_chunk() {
        let mut data = Vec::new();
        data.extend_from_slice(&(42u32).to_le_bytes()); // chunk 42, no flags
        data.extend_from_slice(&5u32.to_le_bytes()); // 32-bit length = 5
        data.extend_from_slice(b"HELLO");
        let chunks = read_chunks(&data).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].chunk_id, 42);
        assert_eq!(chunks[0].length, 5);
        assert_eq!(
            &data[chunks[0].payload_offset..chunks[0].payload_offset + 5],
            b"HELLO"
        );
    }

    #[test]
    fn reads_chunk_with_optional_fields_and_64bit_size() {
        let flags = CHUNK_CALLSTACK
            | CHUNK_THREAD_ID
            | CHUNK_DURATION
            | CHUNK_TIMESTAMP
            | CHUNK_64BIT_SIZE
            | 7;
        let mut data = Vec::new();
        data.extend_from_slice(&flags.to_le_bytes());
        data.extend_from_slice(&2u32.to_le_bytes()); // 2 callstack frames
        data.extend_from_slice(&0x1000u64.to_le_bytes());
        data.extend_from_slice(&0x2000u64.to_le_bytes());
        data.extend_from_slice(&99u32.to_le_bytes()); // thread id
        data.extend_from_slice(&(-5i64).to_le_bytes()); // duration
        data.extend_from_slice(&1234i64.to_le_bytes()); // timestamp
        data.extend_from_slice(&3u64.to_le_bytes()); // 64-bit length
        data.extend_from_slice(b"abc");
        let chunks = read_chunks(&data).unwrap();
        let header = &chunks[0];
        assert_eq!(header.chunk_id, 7);
        assert!(header.has_callstack);
        assert_eq!(header.callstack, vec![0x1000, 0x2000]);
        assert_eq!(header.thread_id, Some(99));
        assert_eq!(header.duration_micro, Some(-5));
        assert_eq!(header.timestamp_micro, Some(1234));
        assert_eq!(header.length, 3);
    }

    #[test]
    fn reads_basic_types_and_strings() {
        let mut data = Vec::new();
        data.extend_from_slice(&7u8.to_le_bytes());
        data.extend_from_slice(&0x1234u32.to_le_bytes());
        data.extend_from_slice(&(-9i64).to_le_bytes());
        data.extend_from_slice(&1.5f32.to_le_bytes());
        data.extend_from_slice(&4i32.to_le_bytes());
        data.extend_from_slice(b"test");
        data.extend_from_slice(&(-1i32).to_le_bytes()); // null string
        let mut cursor = BinaryCursor::new(&data);
        assert_eq!(cursor.u8().unwrap(), 7);
        assert_eq!(cursor.u32().unwrap(), 0x1234);
        assert_eq!(cursor.i64().unwrap(), -9);
        assert!((cursor.f32().unwrap() - 1.5).abs() < 1e-6);
        assert_eq!(cursor.string().unwrap().as_deref(), Some("test"));
        assert_eq!(cursor.string().unwrap(), None);
    }

    #[test]
    fn byte_buffer_applies_64_byte_alignment() {
        // Start at pos=1; a byte buffer must skip to offset 64.
        let mut data = vec![0u8; 70];
        data[64] = 0xAB;
        data[65] = 0xCD;
        let mut cursor = BinaryCursor::new(&data);
        cursor.u8().unwrap(); // pos = 1
        let buffer = cursor.byte_buffer(2).unwrap();
        assert_eq!(buffer, &[0xAB, 0xCD]);
        assert_eq!(cursor.pos, 66);
    }

    #[test]
    fn rejects_truncated_data() {
        assert!(read_chunks(&[1, 2, 3]).is_err());
        let mut cursor = BinaryCursor::new(&[5, 0, 0]);
        assert!(cursor.u32().is_err());
    }
}
