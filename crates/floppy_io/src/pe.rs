//! PE32+ import-table parser for the ship gate (SPEC §12.3). Bounds-checked,
//! dependency-free stand-in for `objdump -p` / dumpbin's import listing: every
//! read is checked and malformed input returns `Err` instead of panicking,
//! since this parses an arbitrary (possibly hand-edited or truncated) file.

struct Section {
    virtual_address: u32,
    virtual_size: u32,
    size_of_raw_data: u32,
    pointer_to_raw_data: u32,
}

fn read_u16(buf: &[u8], offset: usize) -> Result<u16, &'static str> {
    let end = offset.checked_add(2).ok_or("offset overflow")?;
    if end > buf.len() {
        return Err("read out of bounds");
    }
    Ok(u16::from_le_bytes([buf[offset], buf[offset + 1]]))
}

fn read_u32(buf: &[u8], offset: usize) -> Result<u32, &'static str> {
    let end = offset.checked_add(4).ok_or("offset overflow")?;
    if end > buf.len() {
        return Err("read out of bounds");
    }
    Ok(u32::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ]))
}

fn read_cstr(buf: &[u8], offset: usize) -> Result<String, &'static str> {
    if offset >= buf.len() {
        return Err("string offset out of bounds");
    }
    let mut end = offset;
    while end < buf.len() && buf[end] != 0 {
        end += 1;
    }
    if end >= buf.len() {
        return Err("unterminated string");
    }
    let bytes = &buf[offset..end];
    if !bytes.is_ascii() {
        return Err("non-ascii import name");
    }
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

/// Parse the import table of a PE32+ image and return imported DLL names in
/// the order they appear in the import directory.
pub fn imports(image: &[u8]) -> Result<Vec<String>, &'static str> {
    let e_lfanew = read_u32(image, 0x3C)? as usize;

    let sig_end = e_lfanew.checked_add(4).ok_or("offset overflow")?;
    if sig_end > image.len() {
        return Err("PE signature out of bounds");
    }
    if &image[e_lfanew..sig_end] != b"PE\0\0" {
        return Err("missing PE signature");
    }

    let coff = sig_end;
    let number_of_sections =
        read_u16(image, coff.checked_add(2).ok_or("offset overflow")?)? as usize;
    let size_of_optional_header =
        read_u16(image, coff.checked_add(16).ok_or("offset overflow")?)? as usize;

    let optional = coff.checked_add(20).ok_or("offset overflow")?;
    let optional_end = optional
        .checked_add(size_of_optional_header)
        .ok_or("offset overflow")?;
    if optional_end > image.len() {
        return Err("optional header out of bounds");
    }

    let magic = read_u16(image, optional)?;
    if magic != 0x20B {
        return Err("not a PE32+ image (magic mismatch)");
    }

    // Data directory array starts at optional+112; entry 1 is the import table.
    let dir_offset = optional
        .checked_add(112)
        .and_then(|v| v.checked_add(8)) // entry 1 (import table) is 8 bytes into the array
        .ok_or("offset overflow")?;
    let dir_end = dir_offset.checked_add(8).ok_or("offset overflow")?;
    if dir_end > image.len() {
        return Err("data directory out of bounds");
    }
    let import_rva = read_u32(image, dir_offset)?;
    let import_size = read_u32(image, dir_offset + 4)?;

    if import_rva == 0 || import_size == 0 {
        return Ok(Vec::new());
    }

    let section_table = optional
        .checked_add(size_of_optional_header)
        .ok_or("offset overflow")?;
    let table_len = number_of_sections.checked_mul(40).ok_or("overflow")?;
    let table_end = section_table.checked_add(table_len).ok_or("overflow")?;
    if table_end > image.len() {
        return Err("section table out of bounds");
    }

    let mut sections = Vec::with_capacity(number_of_sections);
    for i in 0..number_of_sections {
        let base = section_table + i * 40;
        let virtual_size = read_u32(image, base + 8)?;
        let virtual_address = read_u32(image, base + 12)?;
        let size_of_raw_data = read_u32(image, base + 16)?;
        let pointer_to_raw_data = read_u32(image, base + 20)?;
        sections.push(Section {
            virtual_address,
            virtual_size,
            size_of_raw_data,
            pointer_to_raw_data,
        });
    }

    let rva_to_offset = |rva: u32| -> Result<usize, &'static str> {
        for s in &sections {
            let extent = s.virtual_size.max(s.size_of_raw_data);
            let section_end = s.virtual_address.saturating_add(extent);
            if rva >= s.virtual_address && rva < section_end {
                let delta = (rva - s.virtual_address) as usize;
                return (s.pointer_to_raw_data as usize)
                    .checked_add(delta)
                    .ok_or("rva overflow");
            }
        }
        Err("rva not contained in any section")
    };

    let mut import_off = rva_to_offset(import_rva)?;
    let mut names = Vec::new();
    loop {
        let desc_end = import_off.checked_add(20).ok_or("offset overflow")?;
        if desc_end > image.len() {
            return Err("import descriptor out of bounds");
        }
        let name_rva = read_u32(image, import_off + 12)?;
        if name_rva == 0 {
            break;
        }
        let name_off = rva_to_offset(name_rva)?;
        names.push(read_cstr(image, name_off)?);
        import_off = desc_end;
    }

    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal, synthetic PE32+ image with one section holding an
    /// import directory of two descriptors ("KERNEL32.dll", "user32.dll")
    /// followed by their NUL-terminated names, plus a zero terminator
    /// descriptor.
    fn build_synthetic_pe32plus() -> Vec<u8> {
        let e_lfanew: u32 = 0x40;
        let pe_off = e_lfanew as usize;
        let coff_off = pe_off + 4;
        let opt_off = coff_off + 20;
        let opt_len = 112 + 16 * 8; // full PE32+ optional header (16 data directories)
        let sect_off = opt_off + opt_len;
        let sect_len = 40;
        let raw_off = sect_off + sect_len;

        let section_va: u32 = 0x2000;
        // Layout inside the section, relative to its start:
        let desc0_rel = 0usize;
        let desc1_rel = 20usize;
        let term_rel = 40usize;
        let kernel32_name_rel = 60usize;
        let kernel32_name = b"KERNEL32.dll\0";
        let user32_name_rel = kernel32_name_rel + kernel32_name.len();
        let user32_name = b"user32.dll\0";
        let content_len = user32_name_rel + user32_name.len();

        let total_len = raw_off + content_len;
        let mut img = vec![0u8; total_len];

        // DOS header stub: only e_lfanew at 0x3C matters.
        img[0x3C..0x40].copy_from_slice(&e_lfanew.to_le_bytes());

        // PE signature.
        img[pe_off..pe_off + 4].copy_from_slice(b"PE\0\0");

        // COFF header.
        img[coff_off..coff_off + 2].copy_from_slice(&0x8664u16.to_le_bytes()); // Machine (x64), unchecked
        img[coff_off + 2..coff_off + 4].copy_from_slice(&1u16.to_le_bytes()); // NumberOfSections
        img[coff_off + 16..coff_off + 18].copy_from_slice(&(opt_len as u16).to_le_bytes()); // SizeOfOptionalHeader

        // Optional header: only magic + import data-directory entry matter to the parser.
        img[opt_off..opt_off + 2].copy_from_slice(&0x020Bu16.to_le_bytes()); // PE32+ magic
        let import_rva = section_va + desc0_rel as u32;
        let import_size = (term_rel - desc0_rel + 20) as u32; // two real descriptors + terminator
        let dir_off = opt_off + 112 + 8;
        img[dir_off..dir_off + 4].copy_from_slice(&import_rva.to_le_bytes());
        img[dir_off + 4..dir_off + 8].copy_from_slice(&import_size.to_le_bytes());

        // Section header.
        img[sect_off..sect_off + 8].copy_from_slice(b".idata\0\0");
        img[sect_off + 8..sect_off + 12].copy_from_slice(&(content_len as u32).to_le_bytes()); // VirtualSize
        img[sect_off + 12..sect_off + 16].copy_from_slice(&section_va.to_le_bytes()); // VirtualAddress
        img[sect_off + 16..sect_off + 20].copy_from_slice(&(content_len as u32).to_le_bytes()); // SizeOfRawData
        img[sect_off + 20..sect_off + 24].copy_from_slice(&(raw_off as u32).to_le_bytes()); // PointerToRawData

        // Import descriptor 0 -> KERNEL32.dll
        let kernel32_name_rva = section_va + kernel32_name_rel as u32;
        img[raw_off + desc0_rel + 12..raw_off + desc0_rel + 16]
            .copy_from_slice(&kernel32_name_rva.to_le_bytes());

        // Import descriptor 1 -> user32.dll
        let user32_name_rva = section_va + user32_name_rel as u32;
        img[raw_off + desc1_rel + 12..raw_off + desc1_rel + 16]
            .copy_from_slice(&user32_name_rva.to_le_bytes());

        // Terminator descriptor (desc at term_rel) stays all-zero.
        let _ = term_rel;

        // Name strings.
        img[raw_off + kernel32_name_rel..raw_off + kernel32_name_rel + kernel32_name.len()]
            .copy_from_slice(kernel32_name);
        img[raw_off + user32_name_rel..raw_off + user32_name_rel + user32_name.len()]
            .copy_from_slice(user32_name);

        img
    }

    #[test]
    fn parses_synthetic_import_table() {
        let img = build_synthetic_pe32plus();
        let names = imports(&img).expect("valid synthetic PE should parse");
        assert_eq!(
            names,
            vec!["KERNEL32.dll".to_string(), "user32.dll".to_string()]
        );
    }

    #[test]
    fn empty_input_is_err_not_panic() {
        assert!(imports(&[]).is_err());
    }

    #[test]
    fn short_buffer_is_err_not_panic() {
        assert!(imports(&[0u8; 10]).is_err());
    }

    #[test]
    fn garbage_e_lfanew_is_err_not_panic() {
        let mut img = vec![0u8; 0x100];
        // e_lfanew points wildly out of bounds.
        img[0x3C..0x40].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        assert!(imports(&img).is_err());
    }

    #[test]
    fn wrong_signature_is_err_not_panic() {
        let mut img = vec![0u8; 0x100];
        let e_lfanew: u32 = 0x40;
        img[0x3C..0x40].copy_from_slice(&e_lfanew.to_le_bytes());
        img[0x40..0x44].copy_from_slice(b"NOPE");
        assert!(imports(&img).is_err());
    }

    #[test]
    fn truncated_valid_image_at_every_prefix_is_err_or_ok_never_panics() {
        let img = build_synthetic_pe32plus();
        for len in 0..img.len() {
            // Must not panic for any truncation length; result is irrelevant here.
            let _ = imports(&img[..len]);
        }
    }

    #[test]
    fn pe32_not_plus_is_rejected() {
        let mut img = build_synthetic_pe32plus();
        // Flip the optional header magic to PE32 (0x10B) instead of PE32+.
        let opt_off = 0x40 + 4 + 20;
        img[opt_off..opt_off + 2].copy_from_slice(&0x010Bu16.to_le_bytes());
        assert_eq!(imports(&img), Err("not a PE32+ image (magic mismatch)"));
    }

    #[test]
    fn zero_import_rva_returns_empty() {
        let mut img = build_synthetic_pe32plus();
        let opt_off = 0x40 + 4 + 20;
        let dir_off = opt_off + 112 + 8;
        img[dir_off..dir_off + 4].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(imports(&img), Ok(Vec::new()));
    }
}
