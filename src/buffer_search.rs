//! Streaming byte search over one replay buffer range.
use crate::replay::BUFFER_PAGE_LIMIT;

pub fn parse_hex(text: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if digits.is_empty() || digits.len() % 2 != 0 {
        return None;
    }
    digits
        .chunks_exact(2)
        .map(|pair| {
            let digit = |b: u8| match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                b'A'..=b'F' => Some(b - b'A' + 10),
                _ => None,
            };
            Some(digit(pair[0])? * 16 + digit(pair[1])?)
        })
        .collect()
}

pub fn find<F, C>(
    offset: u64,
    length: u64,
    pattern: &[u8],
    mut read: F,
    check: C,
) -> Result<Option<u64>, String>
where
    F: FnMut(u64, u32) -> Result<Vec<u8>, String>,
    C: Fn() -> Result<(), String>,
{
    let end = offset.checked_add(length).ok_or("Invalid buffer range")?;
    if pattern.is_empty() {
        return Err("Empty byte pattern".into());
    }
    check()?;
    if u64::try_from(pattern.len()).map_or(true, |n| n > length) {
        return Ok(None);
    }
    let mut prefix = vec![0usize; pattern.len()];
    let mut matched = 0;
    for i in 1..pattern.len() {
        while matched > 0 && pattern[i] != pattern[matched] {
            matched = prefix[matched - 1];
        }
        if pattern[i] == pattern[matched] {
            matched += 1;
        }
        prefix[i] = matched;
    }
    matched = 0;
    let mut cursor = offset;
    while cursor < end {
        check()?;
        let count = (end - cursor).min(u64::from(BUFFER_PAGE_LIMIT)) as u32;
        let bytes = read(cursor, count)?;
        if bytes.len() != count as usize {
            return Err("Buffer read returned incomplete data".into());
        }
        for (index, byte) in bytes.into_iter().enumerate() {
            while matched > 0 && byte != pattern[matched] {
                matched = prefix[matched - 1];
            }
            if byte == pattern[matched] {
                matched += 1;
            }
            if matched == pattern.len() {
                check()?;
                return Ok(Some(cursor + index as u64 + 1 - pattern.len() as u64));
            }
        }
        cursor += u64::from(count);
    }
    check()?;
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn searches_across_pages_with_overlaps_and_short_reads() {
        assert_eq!(parse_hex(" 00 aF FF "), Some(vec![0, 175, 255]));
        for bad in ["", "0", "zz", "0x12"] {
            assert_eq!(parse_hex(bad), None);
        }
        let mut data = vec![0; BUFFER_PAGE_LIMIT as usize + 4];
        let n = BUFFER_PAGE_LIMIT as usize;
        data[n - 2..n + 3].copy_from_slice(&[1, 1, 1, 2, 3]);
        let read = |offset: u64, count: u32| {
            Ok(data[offset as usize..offset as usize + count as usize].to_vec())
        };
        assert_eq!(
            find(0, data.len() as u64, &[1, 1, 2, 3], read, || Ok(())).unwrap(),
            Some((n - 1) as u64)
        );
        assert_eq!(
            find(0, data.len() as u64, &[1, 2, 4], read, || Ok(())).unwrap(),
            None
        );
        assert!(find(0, 4, &[1], |_, _| Ok(vec![]), || Ok(())).is_err());
        assert!(find(0, 4, &[], read, || Ok(())).is_err());
        assert!(find(u64::MAX, 1, &[1], read, || Ok(())).is_err());
    }
}
