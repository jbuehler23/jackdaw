//! Binary frame messages for the live frame view. Pixels bypass the JSON
//! codec: a fixed little-endian header followed by tightly packed RGBA8 rows.

/// Decoded frame header plus a borrowed pixel payload.
#[derive(Debug, PartialEq, Eq)]
pub struct FrameRef<'a> {
    pub width: u32,
    pub height: u32,
    pub seq: u64,
    pub pixels: &'a [u8],
}

const MAGIC: &[u8; 4] = b"JDF1";
const HEADER_LEN: usize = 4 + 4 + 4 + 8;

/// Encode a frame message: magic, width, height, seq, then `pixels`.
/// `pixels` must be exactly `width * height * 4` bytes.
pub fn encode_frame(width: u32, height: u32, seq: u64, pixels: &[u8]) -> Vec<u8> {
    debug_assert_eq!(
        Some(pixels.len()),
        (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
    );
    let mut out = Vec::with_capacity(HEADER_LEN + pixels.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(pixels);
    out
}

/// Decode a frame message. Returns `None` on bad magic, short input, or a
/// payload length that does not match the header dimensions.
pub fn decode_frame(bytes: &[u8]) -> Option<FrameRef<'_>> {
    if bytes.len() < HEADER_LEN || &bytes[0..4] != MAGIC {
        return None;
    }
    let width = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let seq = u64::from_le_bytes(bytes[12..20].try_into().ok()?);
    let pixels = &bytes[HEADER_LEN..];
    if pixels.len()
        != (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?
    {
        return None;
    }
    Some(FrameRef {
        width,
        height,
        seq,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trips() {
        let pixels = vec![7u8; 2 * 3 * 4];
        let bytes = encode_frame(2, 3, 42, &pixels);
        let frame = decode_frame(&bytes).expect("decodes");
        assert_eq!(frame.width, 2);
        assert_eq!(frame.height, 3);
        assert_eq!(frame.seq, 42);
        assert_eq!(frame.pixels, &pixels[..]);
    }

    #[test]
    fn decode_rejects_bad_magic_and_truncation() {
        let pixels = vec![0u8; 4];
        let mut bytes = encode_frame(1, 1, 1, &pixels);
        assert!(decode_frame(&bytes[..10]).is_none());
        bytes[0] = b'X';
        assert!(decode_frame(&bytes).is_none());
    }

    #[test]
    fn decode_rejects_length_mismatch() {
        let bytes = encode_frame(1, 1, 1, &[0u8; 4]);
        let mut grown = bytes.clone();
        grown.push(0);
        assert!(decode_frame(&grown).is_none());
    }

    #[test]
    fn zero_size_frame_decodes_and_header_only_nonzero_dims_is_rejected() {
        assert!(decode_frame(&encode_frame(0, 0, 1, &[])).is_some());
        let mut header_only = encode_frame(0, 0, 1, &[]);
        header_only[4..8].copy_from_slice(&2u32.to_le_bytes());
        header_only[8..12].copy_from_slice(&2u32.to_le_bytes());
        assert!(decode_frame(&header_only).is_none());
    }
}
