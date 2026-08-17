//! Synthetic binary fixtures for this crate's in-module tests.
//!
//! Everything here is **built from scratch** in code: header-less DIBs, whole
//! NE (Win16) images, and whole PE32 images with a `.rsrc` resource tree.
//! None of it derives from any real file — no real bytes, not even real
//! dimensions (the cards here are a made-up 5×7). This module is the single
//! source of truth so `dib`, `ne`, `pe`, and `extract` tests share identical
//! builders; it is compiled only under `#[cfg(test)]`.

#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::type_complexity,
    clippy::too_many_lines,
    clippy::cast_possible_truncation
)]

/// Parses `raw` as an asset path, panicking if it is not a valid one.
///
/// Fixtures spell out paths as literals; a literal that does not parse is a
/// broken fixture, not a case under test.
pub fn asset_path(raw: &str) -> sol_theme::RelativeAssetPath {
    sol_theme::RelativeAssetPath::parse("test fixture".to_owned(), raw).unwrap()
}

/// A 40-byte `BITMAPINFOHEADER` with the given fields (planes = 1).
pub fn info_header(
    width: i32,
    height: i32,
    bit_count: u16,
    compression: u32,
    clr_used: u32,
) -> Vec<u8> {
    let mut header = Vec::with_capacity(40);
    header.extend_from_slice(&40_u32.to_le_bytes());
    header.extend_from_slice(&width.to_le_bytes());
    header.extend_from_slice(&height.to_le_bytes());
    header.extend_from_slice(&1_u16.to_le_bytes());
    header.extend_from_slice(&bit_count.to_le_bytes());
    header.extend_from_slice(&compression.to_le_bytes());
    header.extend_from_slice(&0_u32.to_le_bytes());
    header.extend_from_slice(&0_i32.to_le_bytes());
    header.extend_from_slice(&0_i32.to_le_bytes());
    header.extend_from_slice(&clr_used.to_le_bytes());
    header.extend_from_slice(&0_u32.to_le_bytes());
    header
}

/// A header-less 24bpp bottom-up DIB of a solid `(r, g, b)` color, `width` ×
/// `height`. Decodes back to that opaque color for every pixel.
pub fn solid_dib(width: u32, height: u32, color: (u8, u8, u8)) -> Vec<u8> {
    let (red, green, blue) = color;
    let mut bytes = info_header(
        i32::try_from(width).unwrap(),
        i32::try_from(height).unwrap(),
        24,
        0,
        0,
    );
    let stride = ((width * 24).div_ceil(32) * 4) as usize;
    let mut row = Vec::with_capacity(stride);
    for _ in 0..width {
        row.extend_from_slice(&[blue, green, red]);
    }
    row.resize(stride, 0);
    for _ in 0..height {
        bytes.extend_from_slice(&row);
    }
    bytes
}

// ---------------------------------------------------------------------------
// NE (Win16) image builder
// ---------------------------------------------------------------------------

/// Builds a synthetic NE image whose resource table lists `types`, then places
/// each resource's bytes at an aligned position. Returns `(image, ne_offset)`.
/// Each type is `(type_id, entries)`; each entry is `(id, data)`. Layout
/// mirrors the normative NE layout documented on [`crate::ne`].
pub fn build_ne(align_shift: u32, types: &[(u16, Vec<(u16, Vec<u8>)>)]) -> (Vec<u8>, usize) {
    let ne_offset = 0x40_usize;
    let unit = 1_usize << align_shift;

    let mut blocks: Vec<(usize, usize, Vec<u8>)> = Vec::new();
    let mut data_cursor = 0x400_usize;
    for (_type_id, entries) in types {
        for (_id, bytes) in entries {
            let aligned = data_cursor.div_ceil(unit) * unit;
            let offset_units = aligned / unit;
            let length_units = bytes.len().div_ceil(unit).max(1);
            blocks.push((offset_units, length_units, bytes.clone()));
            data_cursor = aligned + length_units * unit;
        }
    }

    let mut table = Vec::new();
    table.extend_from_slice(&u16::try_from(align_shift).unwrap().to_le_bytes());
    let mut block_index = 0;
    for (type_id, entries) in types {
        table.extend_from_slice(&type_id.to_le_bytes());
        table.extend_from_slice(&u16::try_from(entries.len()).unwrap().to_le_bytes());
        table.extend_from_slice(&0_u32.to_le_bytes());
        for (id, _bytes) in entries {
            let (offset_units, length_units, _) = &blocks[block_index];
            block_index += 1;
            table.extend_from_slice(&u16::try_from(*offset_units).unwrap().to_le_bytes());
            table.extend_from_slice(&u16::try_from(*length_units).unwrap().to_le_bytes());
            table.extend_from_slice(&0_u16.to_le_bytes());
            table.extend_from_slice(&id.to_le_bytes());
            table.extend_from_slice(&0_u16.to_le_bytes());
            table.extend_from_slice(&0_u16.to_le_bytes());
        }
    }
    table.extend_from_slice(&0_u16.to_le_bytes());

    let table_start = ne_offset + 0x100;
    let mut image = vec![0_u8; data_cursor];
    image[0..2].copy_from_slice(b"MZ");
    image[0x3C..0x40].copy_from_slice(&u32::try_from(ne_offset).unwrap().to_le_bytes());
    image[ne_offset..ne_offset + 2].copy_from_slice(b"NE");
    let table_relative = u16::try_from(table_start - ne_offset).unwrap();
    image[ne_offset + 0x24..ne_offset + 0x26].copy_from_slice(&table_relative.to_le_bytes());
    image[table_start..table_start + table.len()].copy_from_slice(&table);

    for (offset_units, _length_units, bytes) in &blocks {
        let start = offset_units * unit;
        image[start..start + bytes.len()].copy_from_slice(bytes);
    }

    (image, ne_offset)
}

/// An NE image whose `RT_BITMAP` table holds `count` `NAMEINFO` records that
/// each claim the same `span` bytes at the start of the file.
///
/// Every record is individually in bounds, so nothing but an aggregate budget
/// can refuse it — which is the point: the file stays small while the bytes
/// it asks the reader to copy grow with `count`.
pub fn build_ne_repeated_full_span(count: u16, span: usize) -> (Vec<u8>, usize) {
    const RT_BITMAP: u16 = 0x8002;
    const INTEGER_ID_FLAG: u16 = 0x8000;

    let ne_offset = 0x40_usize;
    let table_start = ne_offset + 0x100;

    let mut table = Vec::new();
    table.extend_from_slice(&0_u16.to_le_bytes()); // align_shift: bytes are units
    table.extend_from_slice(&RT_BITMAP.to_le_bytes());
    table.extend_from_slice(&count.to_le_bytes());
    table.extend_from_slice(&0_u32.to_le_bytes()); // reserved
    for index in 0..count {
        table.extend_from_slice(&0_u16.to_le_bytes()); // offset: file start
        table.extend_from_slice(&u16::try_from(span).unwrap().to_le_bytes());
        table.extend_from_slice(&0_u16.to_le_bytes()); // flags
        table.extend_from_slice(&(index | INTEGER_ID_FLAG).to_le_bytes());
        table.extend_from_slice(&0_u16.to_le_bytes()); // handle
        table.extend_from_slice(&0_u16.to_le_bytes()); // usage
    }
    table.extend_from_slice(&0_u16.to_le_bytes()); // TYPEINFO terminator

    let mut image = vec![0_u8; (table_start + table.len()).max(span)];
    image[0..2].copy_from_slice(b"MZ");
    image[0x3C..0x40].copy_from_slice(&u32::try_from(ne_offset).unwrap().to_le_bytes());
    image[ne_offset..ne_offset + 2].copy_from_slice(b"NE");
    let table_relative = u16::try_from(table_start - ne_offset).unwrap();
    image[ne_offset + 0x24..ne_offset + 0x26].copy_from_slice(&table_relative.to_le_bytes());
    image[table_start..table_start + table.len()].copy_from_slice(&table);

    (image, ne_offset)
}

// ---------------------------------------------------------------------------
// PE32 image builder (a `.rsrc` tree pelite parses)
// ---------------------------------------------------------------------------

/// One resource under a type, for [`build_pe`].
pub enum Rsrc {
    /// Integer id nesting a language (0) directory, then the data — the
    /// standard three-level `type / id / language` PE resource shape.
    Id(u32, Vec<u8>),
    /// Integer id pointing straight at a data entry (a two-level tree).
    IdDirect(u32, Vec<u8>),
    /// A UTF-16 string-named resource nesting a language directory.
    Named(&'static str, Vec<u8>),
}

/// The resource virtual address the builder places `.rsrc` at.
const RSRC_RVA: u32 = 0x1000;

/// Builds a minimal PE32 image whose `.rsrc` section holds `types` (each a
/// `(type_id, entries)`), laid out as a resource directory tree pelite parses.
pub fn build_pe(types: &[(u16, Vec<Rsrc>)]) -> Vec<u8> {
    let rsrc = build_rsrc(types);
    wrap_pe32(&rsrc)
}

/// A node in the resource tree during layout.
struct DataLeaf {
    /// Byte offset of the `IMAGE_RESOURCE_DATA_ENTRY` struct within `.rsrc`.
    entry_offset: u32,
    /// Byte offset of the blob within `.rsrc`.
    blob_offset: u32,
    blob: Vec<u8>,
}

/// Emits the `.rsrc` section bytes for `types`.
///
/// Layout order (each region 4-aligned where its structs require it): every
/// directory + its entry array, then every `IMAGE_RESOURCE_DATA_ENTRY`, then
/// every name string, then every data blob. Offsets into directories/entries
/// are `.rsrc`-relative; a data entry's `OffsetToData` is an absolute RVA.
fn build_rsrc(types: &[(u16, Vec<Rsrc>)]) -> Vec<u8> {
    // Pass 1: assign offsets. Directories first (root, type dirs, language
    // dirs), then data-entry structs, then name strings, then blobs.
    let dir_bytes = |entry_count: usize| 16 + entry_count * 8;

    let mut cursor = 0u32;
    let root_offset = cursor;
    cursor += u32::try_from(dir_bytes(types.len())).unwrap();

    // Type directories (one per type).
    let mut type_dir_offsets = Vec::new();
    for (_id, entries) in types {
        type_dir_offsets.push(cursor);
        cursor += u32::try_from(dir_bytes(entries.len())).unwrap();
    }

    // Language directories: one per resource that nests a language level.
    let mut lang_dir_offsets: Vec<Vec<Option<u32>>> = Vec::new();
    for (_id, entries) in types {
        let mut per_type = Vec::new();
        for entry in entries {
            match entry {
                Rsrc::Id(..) | Rsrc::Named(..) => {
                    per_type.push(Some(cursor));
                    cursor += u32::try_from(dir_bytes(1)).unwrap();
                }
                Rsrc::IdDirect(..) => per_type.push(None),
            }
        }
        lang_dir_offsets.push(per_type);
    }

    // Data-entry structs (one leaf per resource).
    let mut leaves: Vec<Vec<DataLeaf>> = Vec::new();
    for (_id, entries) in types {
        let mut per_type = Vec::new();
        for entry in entries {
            let entry_offset = cursor;
            cursor += 16;
            per_type.push(DataLeaf {
                entry_offset,
                blob_offset: 0,
                blob: entry.blob(),
            });
        }
        leaves.push(per_type);
    }

    // Name strings (2-byte length prefix + UTF-16), kept even-aligned.
    let mut name_offsets: Vec<Vec<Option<u32>>> = Vec::new();
    let mut name_blobs: Vec<(u32, Vec<u8>)> = Vec::new();
    for (_id, entries) in types {
        let mut per_type = Vec::new();
        for entry in entries {
            if let Rsrc::Named(name, _) = entry {
                let offset = cursor;
                let units: Vec<u16> = name.encode_utf16().collect();
                let mut bytes = Vec::new();
                bytes.extend_from_slice(&u16::try_from(units.len()).unwrap().to_le_bytes());
                for unit in units {
                    bytes.extend_from_slice(&unit.to_le_bytes());
                }
                cursor += u32::try_from(bytes.len()).unwrap();
                name_blobs.push((offset, bytes));
                per_type.push(Some(offset));
            } else {
                per_type.push(None);
            }
        }
        name_offsets.push(per_type);
    }

    // Data blobs (byte-read; no alignment requirement).
    for per_type in &mut leaves {
        for leaf in per_type {
            leaf.blob_offset = cursor;
            cursor += u32::try_from(leaf.blob.len()).unwrap();
        }
    }

    // Pass 2: emit. Start with the tree of directories.
    let total = cursor as usize;
    let mut rsrc = vec![0u8; total];

    // Root directory: entries are the types (all id entries).
    write_directory(
        &mut rsrc,
        root_offset,
        0,
        types.len(),
        types.iter().enumerate().map(|(i, (type_id, _))| DirEntry {
            name: NameField::Id(u32::from(*type_id)),
            child: ChildRef::Subdir(type_dir_offsets[i]),
        }),
    );

    // Type directories.
    for (type_index, (_type_id, entries)) in types.iter().enumerate() {
        let named_count = entries
            .iter()
            .filter(|e| matches!(e, Rsrc::Named(..)))
            .count();
        // Named entries must precede id entries in the array.
        let mut ordered: Vec<DirEntry> = Vec::new();
        for (entry_index, entry) in entries.iter().enumerate() {
            if let Rsrc::Named(..) = entry {
                ordered.push(dir_entry_for(
                    entry,
                    name_offsets[type_index][entry_index],
                    lang_dir_offsets[type_index][entry_index],
                    &leaves[type_index][entry_index],
                ));
            }
        }
        for (entry_index, entry) in entries.iter().enumerate() {
            if !matches!(entry, Rsrc::Named(..)) {
                ordered.push(dir_entry_for(
                    entry,
                    name_offsets[type_index][entry_index],
                    lang_dir_offsets[type_index][entry_index],
                    &leaves[type_index][entry_index],
                ));
            }
        }
        write_directory(
            &mut rsrc,
            type_dir_offsets[type_index],
            named_count,
            entries.len() - named_count,
            ordered.into_iter(),
        );
    }

    // Language directories (single id-0 entry pointing at the data entry).
    for (type_index, entries) in types.iter().map(|(_, e)| e).enumerate() {
        for (entry_index, _entry) in entries.iter().enumerate() {
            if let Some(lang_offset) = lang_dir_offsets[type_index][entry_index] {
                let leaf = &leaves[type_index][entry_index];
                write_directory(
                    &mut rsrc,
                    lang_offset,
                    0,
                    1,
                    std::iter::once(DirEntry {
                        name: NameField::Id(0),
                        child: ChildRef::Data(leaf.entry_offset),
                    }),
                );
            }
        }
    }

    // Data-entry structs.
    for per_type in &leaves {
        for leaf in per_type {
            let rva = RSRC_RVA + leaf.blob_offset;
            let at = leaf.entry_offset as usize;
            rsrc[at..at + 4].copy_from_slice(&rva.to_le_bytes());
            rsrc[at + 4..at + 8]
                .copy_from_slice(&u32::try_from(leaf.blob.len()).unwrap().to_le_bytes());
            // CodePage + Reserved already zero.
        }
    }

    // Name strings.
    for (offset, bytes) in &name_blobs {
        let at = *offset as usize;
        rsrc[at..at + bytes.len()].copy_from_slice(bytes);
    }

    // Data blobs.
    for per_type in &leaves {
        for leaf in per_type {
            let at = leaf.blob_offset as usize;
            rsrc[at..at + leaf.blob.len()].copy_from_slice(&leaf.blob);
        }
    }

    rsrc
}

impl Rsrc {
    fn blob(&self) -> Vec<u8> {
        match self {
            Rsrc::Id(_, blob) | Rsrc::IdDirect(_, blob) | Rsrc::Named(_, blob) => blob.clone(),
        }
    }
}

/// The `Name` field of a directory entry.
enum NameField {
    Id(u32),
    NameString(u32),
}

/// What a directory entry points at.
enum ChildRef {
    Subdir(u32),
    Data(u32),
}

/// A directory entry to emit.
struct DirEntry {
    name: NameField,
    child: ChildRef,
}

/// High bit marking a directory-entry offset as a subdirectory / a name field
/// as a string offset.
const HIGH_BIT: u32 = 0x8000_0000;

/// Resolves one `Rsrc` to its `DirEntry` under its type directory.
fn dir_entry_for(
    entry: &Rsrc,
    name_offset: Option<u32>,
    lang_offset: Option<u32>,
    leaf: &DataLeaf,
) -> DirEntry {
    let name = match entry {
        Rsrc::Id(id, _) | Rsrc::IdDirect(id, _) => NameField::Id(*id),
        Rsrc::Named(..) => NameField::NameString(name_offset.unwrap()),
    };
    let child = match lang_offset {
        Some(offset) => ChildRef::Subdir(offset),
        None => ChildRef::Data(leaf.entry_offset),
    };
    DirEntry { name, child }
}

/// Writes an `IMAGE_RESOURCE_DIRECTORY` header plus its entry array at
/// `offset`.
fn write_directory(
    rsrc: &mut [u8],
    offset: u32,
    named_count: usize,
    id_count: usize,
    entries: impl Iterator<Item = DirEntry>,
) {
    let at = offset as usize;
    // Characteristics, TimeDateStamp, versions all zero.
    rsrc[at + 12..at + 14].copy_from_slice(&u16::try_from(named_count).unwrap().to_le_bytes());
    rsrc[at + 14..at + 16].copy_from_slice(&u16::try_from(id_count).unwrap().to_le_bytes());
    let mut entry_at = at + 16;
    for entry in entries {
        let name = match entry.name {
            NameField::Id(id) => id,
            NameField::NameString(off) => off | HIGH_BIT,
        };
        let child = match entry.child {
            ChildRef::Subdir(off) => off | HIGH_BIT,
            ChildRef::Data(off) => off,
        };
        rsrc[entry_at..entry_at + 4].copy_from_slice(&name.to_le_bytes());
        rsrc[entry_at + 4..entry_at + 8].copy_from_slice(&child.to_le_bytes());
        entry_at += 8;
    }
}

/// Wraps `.rsrc` section bytes in a minimal, pelite-parseable PE32 image.
fn wrap_pe32(rsrc: &[u8]) -> Vec<u8> {
    const FILE_ALIGN: u32 = 0x200;
    const SECTION_ALIGN: u32 = 0x1000;
    let align = |value: u32, to: u32| value.div_ceil(to) * to;

    let rsrc_len = u32::try_from(rsrc.len()).unwrap();
    let headers_raw = FILE_ALIGN; // 0x200: everything up to here is headers
    let rsrc_raw_len = align(rsrc_len, FILE_ALIGN);
    let size_of_image = SECTION_ALIGN + align(rsrc_len, SECTION_ALIGN);

    let mut image = vec![0u8; (headers_raw + rsrc_raw_len) as usize];

    // DOS header.
    image[0..2].copy_from_slice(b"MZ");
    let pe_offset = 0x40u32;
    image[0x3C..0x40].copy_from_slice(&pe_offset.to_le_bytes());

    // PE signature.
    let pe = pe_offset as usize;
    image[pe..pe + 4].copy_from_slice(b"PE\0\0");

    // COFF file header.
    let coff = pe + 4;
    image[coff..coff + 2].copy_from_slice(&0x014Cu16.to_le_bytes()); // Machine i386
    image[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // NumberOfSections
    image[coff + 16..coff + 18].copy_from_slice(&0xE0u16.to_le_bytes()); // SizeOfOptionalHeader
    image[coff + 18..coff + 20].copy_from_slice(&0x0102u16.to_le_bytes()); // Characteristics

    // Optional header (PE32).
    let opt = coff + 20;
    image[opt..opt + 2].copy_from_slice(&0x010Bu16.to_le_bytes()); // Magic PE32
    image[opt + 16..opt + 20].copy_from_slice(&0u32.to_le_bytes()); // AddressOfEntryPoint
    image[opt + 28..opt + 32].copy_from_slice(&0x0040_0000u32.to_le_bytes()); // ImageBase
    image[opt + 32..opt + 36].copy_from_slice(&SECTION_ALIGN.to_le_bytes());
    image[opt + 36..opt + 40].copy_from_slice(&FILE_ALIGN.to_le_bytes());
    image[opt + 40..opt + 42].copy_from_slice(&4u16.to_le_bytes()); // MajorOSVersion
    image[opt + 48..opt + 50].copy_from_slice(&4u16.to_le_bytes()); // MajorSubsystemVersion
    image[opt + 56..opt + 60].copy_from_slice(&size_of_image.to_le_bytes());
    image[opt + 60..opt + 64].copy_from_slice(&headers_raw.to_le_bytes()); // SizeOfHeaders
    image[opt + 68..opt + 70].copy_from_slice(&2u16.to_le_bytes()); // Subsystem = GUI
    image[opt + 92..opt + 96].copy_from_slice(&16u32.to_le_bytes()); // NumberOfRvaAndSizes

    // Data directory 2 = Resource Table. Each entry is 8 bytes starting at
    // opt + 96; index 2 is at opt + 96 + 16.
    let resource_dir = opt + 96 + 2 * 8;
    image[resource_dir..resource_dir + 4].copy_from_slice(&RSRC_RVA.to_le_bytes());
    image[resource_dir + 4..resource_dir + 8].copy_from_slice(&rsrc_len.to_le_bytes());

    // Section header (.rsrc) right after the optional header.
    let section = opt + 0xE0;
    image[section..section + 5].copy_from_slice(b".rsrc");
    image[section + 8..section + 12].copy_from_slice(&rsrc_len.to_le_bytes()); // VirtualSize
    image[section + 12..section + 16].copy_from_slice(&RSRC_RVA.to_le_bytes()); // VirtualAddress
    image[section + 16..section + 20].copy_from_slice(&rsrc_raw_len.to_le_bytes()); // SizeOfRawData
    image[section + 20..section + 24].copy_from_slice(&headers_raw.to_le_bytes()); // PointerToRawData
    image[section + 36..section + 40].copy_from_slice(&0x4000_0040u32.to_le_bytes()); // INITIALIZED_DATA | READ

    // Raw .rsrc data at PointerToRawData.
    let raw = headers_raw as usize;
    image[raw..raw + rsrc.len()].copy_from_slice(rsrc);

    image
}
