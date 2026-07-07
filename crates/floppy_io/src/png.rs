//! Minimal PNG encoder for headless verification frames (SPEC C6).
//!
//! Emits 8-bit truecolor (RGB) PNGs. The IDAT payload is a zlib stream built
//! from uncompressed ("stored") DEFLATE blocks only — no real compression is
//! needed for a verification artifact, and it keeps this encoder dependency-free
//! and trivially correct to re-derive by hand when auditing golden frames.

const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// Encode `width * height` pixels (row-major, `0x00RRGGBB` each) as an 8-bit
/// truecolor PNG.
pub fn encode_rgb(width: u32, height: u32, pixels: &[u32]) -> Vec<u8> {
    assert_eq!(
        pixels.len(),
        width as usize * height as usize,
        "pixel buffer length must equal width*height"
    );

    // Filtered scanlines: one leading filter-type byte (0 = None) per row,
    // followed by 3 bytes (R,G,B) per pixel.
    let stride = width as usize * 3;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0u8); // filter type: None
        for x in 0..width as usize {
            let p = pixels[y * width as usize + x];
            raw.push(((p >> 16) & 0xFF) as u8); // R
            raw.push(((p >> 8) & 0xFF) as u8); // G
            raw.push((p & 0xFF) as u8); // B
        }
    }

    let mut out = Vec::new();
    out.extend_from_slice(&PNG_SIGNATURE);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(2); // color type: truecolor (RGB)
    ihdr.push(0); // compression method
    ihdr.push(0); // filter method
    ihdr.push(0); // interlace method
    write_chunk(&mut out, b"IHDR", &ihdr);

    let zlib = zlib_store(&raw);
    write_chunk(&mut out, b"IDAT", &zlib);

    write_chunk(&mut out, b"IEND", &[]);

    out
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);

    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// Wrap `data` in a zlib stream (RFC 1950) made of uncompressed DEFLATE
/// (RFC 1951) stored blocks, each up to 65535 bytes.
fn zlib_store(data: &[u8]) -> Vec<u8> {
    const MAX_BLOCK: usize = 65535;

    let mut out = Vec::with_capacity(data.len() + (data.len() / MAX_BLOCK + 1) * 5 + 6);
    out.push(0x78); // CMF: deflate, 32k window
    out.push(0x01); // FLG: check bits, no dict, fastest level (FCHECK makes 0x7801 % 31 == 0)

    if data.is_empty() {
        // A single empty final stored block.
        out.push(0x01);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    } else {
        let mut offset = 0usize;
        while offset < data.len() {
            let remaining = data.len() - offset;
            let block_len = remaining.min(MAX_BLOCK);
            let is_final = offset + block_len == data.len();

            out.push(if is_final { 0x01 } else { 0x00 }); // BFINAL + BTYPE=00, byte-aligned
            let len = block_len as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(&data[offset..offset + block_len]);

            offset += block_len;
        }
    }

    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Decode a PNG produced by [`encode_rgb`] — and ONLY that subset: 8-bit
/// truecolor (color type 2), filter type `0` (None) on every scanline, and
/// zlib/DEFLATE data made entirely of uncompressed ("stored") blocks. Every
/// read is bounds-checked (never panics/indexes out of range); anything
/// outside the supported subset (wrong signature, bit depth, color type,
/// filter byte, a compressed/fixed/dynamic-Huffman DEFLATE block, a bad
/// stored-block length pair, a Ihdr with a zero dimension, a truncated
/// buffer, ...) returns `Err` with a fixed diagnostic string (SPEC C6:
/// golden-frame verification round-trips through this).
pub fn decode_rgb(png: &[u8]) -> Result<(u32, u32, Vec<u32>), &'static str> {
    if png.len() < PNG_SIGNATURE.len() || png[..PNG_SIGNATURE.len()] != PNG_SIGNATURE {
        return Err("bad PNG signature");
    }

    let mut offset = PNG_SIGNATURE.len();
    let mut width: u32 = 0;
    let mut height: u32 = 0;
    let mut have_ihdr = false;
    let mut idat: Vec<u8> = Vec::new();
    let mut have_iend = false;

    while offset < png.len() {
        if offset + 8 > png.len() {
            return Err("truncated chunk header");
        }
        let len = u32::from_be_bytes(png[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = &png[offset + 4..offset + 8];
        let data_start = offset + 8;
        let data_end = match data_start.checked_add(len) {
            Some(e) => e,
            None => return Err("chunk length overflow"),
        };
        if data_end + 4 > png.len() {
            return Err("truncated chunk data/crc");
        }
        let data = &png[data_start..data_end];
        let stored_crc = u32::from_be_bytes(png[data_end..data_end + 4].try_into().unwrap());
        let mut crc_input = Vec::with_capacity(4 + len);
        crc_input.extend_from_slice(kind);
        crc_input.extend_from_slice(data);
        if crc32(&crc_input) != stored_crc {
            return Err("chunk CRC mismatch");
        }

        match kind {
            b"IHDR" => {
                if data.len() != 13 {
                    return Err("malformed IHDR");
                }
                width = u32::from_be_bytes(data[0..4].try_into().unwrap());
                height = u32::from_be_bytes(data[4..8].try_into().unwrap());
                let bit_depth = data[8];
                let color_type = data[9];
                let compression = data[10];
                let filter_method = data[11];
                let interlace = data[12];
                if width == 0 || height == 0 {
                    return Err("zero image dimension");
                }
                if bit_depth != 8 {
                    return Err("unsupported bit depth (only 8 supported)");
                }
                if color_type != 2 {
                    return Err("unsupported color type (only truecolor=2 supported)");
                }
                if compression != 0 || filter_method != 0 || interlace != 0 {
                    return Err("unsupported IHDR compression/filter/interlace method");
                }
                have_ihdr = true;
            }
            b"IDAT" => {
                if !have_ihdr {
                    return Err("IDAT before IHDR");
                }
                idat.extend_from_slice(data);
            }
            b"IEND" => {
                have_iend = true;
                offset = data_end + 4;
                break;
            }
            _ => {} // ancillary chunks are skipped, matching encode_rgb's minimal output
        }

        offset = data_end + 4;
    }

    if !have_ihdr {
        return Err("missing IHDR");
    }
    if !have_iend {
        return Err("missing IEND");
    }
    let _ = offset;

    let raw = inflate_stored(&idat)?;

    let stride = width as usize * 3;
    let expected_len = (stride + 1) * height as usize;
    if raw.len() != expected_len {
        return Err("decompressed size does not match width/height");
    }

    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for y in 0..height as usize {
        let row_start = y * (stride + 1);
        let filter_type = raw[row_start];
        if filter_type != 0 {
            return Err("unsupported scanline filter (only None=0 supported)");
        }
        let row = &raw[row_start + 1..row_start + 1 + stride];
        for x in 0..width as usize {
            let px = &row[x * 3..x * 3 + 3];
            let p = ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | (px[2] as u32);
            pixels.push(p);
        }
    }

    Ok((width, height, pixels))
}

/// Inflate a zlib stream (RFC 1950) made ENTIRELY of uncompressed ("stored")
/// DEFLATE (RFC 1951) blocks — the only encoding [`zlib_store`] ever
/// produces. Bounds-checked; `Err` on anything else (a fixed/dynamic-
/// Huffman block, a preset dictionary, a stored-block length/complement
/// mismatch, a bad Adler-32 trailer, or a truncated stream).
fn inflate_stored(zlib: &[u8]) -> Result<Vec<u8>, &'static str> {
    if zlib.len() < 6 {
        return Err("zlib stream too short");
    }
    let cmf = zlib[0];
    let flg = zlib[1];
    if cmf & 0x0F != 8 {
        return Err("unsupported zlib compression method (only DEFLATE=8 supported)");
    }
    if flg & 0x20 != 0 {
        return Err("unsupported zlib preset dictionary");
    }
    if !(((cmf as u16) << 8) | flg as u16).is_multiple_of(31) {
        return Err("zlib header check bits invalid");
    }

    let body = &zlib[2..zlib.len() - 4];
    let trailer = &zlib[zlib.len() - 4..];

    let mut out = Vec::new();
    let mut pos = 0usize;
    loop {
        if pos >= body.len() {
            return Err("truncated deflate stream (no final block)");
        }
        let header = body[pos];
        pos += 1;
        if header & 0xFE != 0 {
            return Err("unsupported deflate block type (only stored=00 supported)");
        }
        let is_final = header & 1 != 0;

        if pos + 4 > body.len() {
            return Err("truncated stored-block length header");
        }
        let len = u16::from_le_bytes([body[pos], body[pos + 1]]);
        let nlen = u16::from_le_bytes([body[pos + 2], body[pos + 3]]);
        if nlen != !len {
            return Err("stored-block LEN/NLEN mismatch");
        }
        pos += 4;

        let len = len as usize;
        if pos + len > body.len() {
            return Err("truncated stored-block data");
        }
        out.extend_from_slice(&body[pos..pos + len]);
        pos += len;

        if is_final {
            break;
        }
    }

    if trailer.len() != 4 {
        return Err("truncated zlib Adler-32 trailer");
    }
    let stored_adler = u32::from_be_bytes(trailer.try_into().unwrap());
    if adler32(&out) != stored_adler {
        return Err("zlib Adler-32 checksum mismatch");
    }

    Ok(out)
}

const fn build_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0usize;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            if c & 1 != 0 {
                c = 0xEDB88320 ^ (c >> 1);
            } else {
                c >>= 1;
            }
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

const CRC32_TABLE: [u32; 256] = build_crc32_table();

/// Standard CRC-32 (IEEE 802.3), polynomial 0xEDB88320, as used by PNG chunks.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in bytes {
        let idx = ((crc ^ b as u32) & 0xFF) as usize;
        crc = CRC32_TABLE[idx] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// Adler-32 checksum as used by the zlib trailer.
fn adler32(bytes: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65521;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in bytes {
        a = (a + byte as u32) % MOD_ADLER;
        b = (b + a) % MOD_ADLER;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler32_known_value() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn crc32_known_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn small_png_structure_and_checksums() {
        let width = 3u32;
        let height = 2u32;
        let pixels: Vec<u32> = (0..width * height).map(|i| i * 0x010101).collect();
        let png = encode_rgb(width, height, &pixels);

        assert_eq!(&png[0..8], &PNG_SIGNATURE);

        let mut offset = 8usize;
        let mut chunks = Vec::new();
        loop {
            assert!(offset + 8 <= png.len(), "truncated chunk header");
            let len = u32::from_be_bytes(png[offset..offset + 4].try_into().unwrap()) as usize;
            let kind = &png[offset + 4..offset + 8];
            let data_start = offset + 8;
            let data_end = data_start + len;
            assert!(data_end + 4 <= png.len(), "truncated chunk data/crc");
            let data = &png[data_start..data_end];
            let stored_crc = u32::from_be_bytes(png[data_end..data_end + 4].try_into().unwrap());

            let mut crc_input = Vec::with_capacity(4 + len);
            crc_input.extend_from_slice(kind);
            crc_input.extend_from_slice(data);
            assert_eq!(
                crc32(&crc_input),
                stored_crc,
                "CRC mismatch for chunk {kind:?}"
            );

            chunks.push((kind.to_vec(), data.to_vec()));
            offset = data_end + 4;

            if kind == b"IEND" {
                break;
            }
        }

        assert_eq!(offset, png.len(), "trailing bytes after IEND");

        assert_eq!(chunks[0].0, b"IHDR");
        let ihdr = &chunks[0].1;
        assert_eq!(u32::from_be_bytes(ihdr[0..4].try_into().unwrap()), width);
        assert_eq!(u32::from_be_bytes(ihdr[4..8].try_into().unwrap()), height);
        assert_eq!(ihdr[8], 8); // bit depth
        assert_eq!(ihdr[9], 2); // color type: truecolor

        assert_eq!(chunks.last().unwrap().0, b"IEND");
        assert!(chunks.last().unwrap().1.is_empty());

        assert!(chunks.iter().any(|(k, _)| k == b"IDAT"));
    }

    #[test]
    fn decode_rgb_round_trips_through_encode_rgb() {
        let width = 17u32;
        let height = 11u32;
        let pixels: Vec<u32> = (0..width * height)
            .map(|i| ((i * 37) & 0xFF) | (((i * 53) & 0xFF) << 8) | (((i * 11) & 0xFF) << 16))
            .collect();
        let png = encode_rgb(width, height, &pixels);
        let (got_w, got_h, got_pixels) = decode_rgb(&png).expect("decode should succeed");
        assert_eq!(got_w, width);
        assert_eq!(got_h, height);
        assert_eq!(got_pixels, pixels);
    }

    #[test]
    fn decode_rgb_round_trips_a_multi_block_image() {
        // Force `zlib_store` to emit more than one stored block (MAX_BLOCK =
        // 65535 bytes) so the round trip also exercises the multi-block
        // stored-deflate path, not just a single small block.
        let width = 300u32;
        let height = 100u32; // stride+1 = 901 bytes/row * 100 rows > 65535
                             // Masked to 0x00RRGGBB: encode_rgb only ever keeps the low 24 bits
                             // of each pixel, so comparing against an unmasked value would be a
                             // test bug, not a real encode/decode mismatch.
        let pixels: Vec<u32> = (0..width * height)
            .map(|i| i.wrapping_mul(2654435761) & 0x00FF_FFFF)
            .collect();
        let png = encode_rgb(width, height, &pixels);
        let (got_w, got_h, got_pixels) = decode_rgb(&png).expect("decode should succeed");
        assert_eq!(got_w, width);
        assert_eq!(got_h, height);
        assert_eq!(got_pixels, pixels);
    }

    #[test]
    fn decode_rgb_round_trips_a_single_pixel_image() {
        let png = encode_rgb(1, 1, &[0x00ABCDEF]);
        let (w, h, pixels) = decode_rgb(&png).unwrap();
        assert_eq!((w, h), (1, 1));
        assert_eq!(pixels, vec![0x00ABCDEF]);
    }

    #[test]
    fn decode_rgb_rejects_bad_signature() {
        let mut png = encode_rgb(2, 2, &[0, 0, 0, 0]);
        png[0] = 0x00;
        assert!(decode_rgb(&png).is_err());
    }

    #[test]
    fn decode_rgb_rejects_truncated_input() {
        let png = encode_rgb(2, 2, &[0, 0, 0, 0]);
        for cut in [0usize, 4, 8, 20] {
            let truncated = &png[..png.len().saturating_sub(cut).min(png.len() - 1)];
            assert!(
                decode_rgb(truncated).is_err(),
                "expected an error for a truncated buffer (cut={cut})"
            );
        }
        assert!(decode_rgb(&[]).is_err());
    }

    #[test]
    fn decode_rgb_rejects_wrong_bit_depth_or_color_type() {
        let png = encode_rgb(2, 2, &[0, 0, 0, 0]);
        // IHDR data starts right after the 8-byte signature + 8-byte chunk
        // header (length + "IHDR"): width(4) height(4) bit_depth(1)
        // color_type(1) ...
        let ihdr_data_start = 8 + 8;
        let bit_depth_offset = ihdr_data_start + 8;
        let color_type_offset = ihdr_data_start + 9;

        let mut bad_depth = png.clone();
        bad_depth[bit_depth_offset] = 16;
        // The CRC no longer matches, but a corrupted bit depth must be
        // rejected regardless of which check fires first.
        assert!(decode_rgb(&bad_depth).is_err());

        let mut bad_color = png.clone();
        bad_color[color_type_offset] = 6; // RGBA
        assert!(decode_rgb(&bad_color).is_err());
    }

    #[test]
    fn decode_rgb_rejects_corrupted_chunk_crc() {
        let mut png = encode_rgb(2, 2, &[0x00112233, 0x00445566, 0x00778899, 0x00AABBCC]);
        let last = png.len() - 1;
        png[last] ^= 0xFF; // flip a bit in the IEND CRC
        assert!(decode_rgb(&png).is_err());
    }

    #[test]
    fn decode_rgb_rejects_corrupted_pixel_data() {
        let mut png = encode_rgb(4, 4, &[0x00808080; 16]);
        // Flip a byte inside the IDAT chunk's stored-block payload without
        // fixing up the CRC/Adler32 — must be caught, not silently decoded.
        let idat_pos = find_chunk_data_offset(&png, b"IDAT").expect("has IDAT");
        png[idat_pos + 10] ^= 0xFF;
        assert!(decode_rgb(&png).is_err());
    }

    /// Test-only helper: locate the data offset of the first chunk with the
    /// given 4-byte type, by walking the same chunk framing decode_rgb uses.
    fn find_chunk_data_offset(png: &[u8], kind: &[u8; 4]) -> Option<usize> {
        let mut offset = PNG_SIGNATURE.len();
        while offset + 8 <= png.len() {
            let len = u32::from_be_bytes(png[offset..offset + 4].try_into().unwrap()) as usize;
            let this_kind = &png[offset + 4..offset + 8];
            let data_start = offset + 8;
            let data_end = data_start + len;
            if this_kind == kind {
                return Some(data_start);
            }
            offset = data_end + 4;
        }
        None
    }
}
