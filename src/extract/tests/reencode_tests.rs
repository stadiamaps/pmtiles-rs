// Tests ported from go-pmtiles/pmtiles/extract_test.go
// TestReencodeEntries* functions (lines 80-136)

use crate::directory::DirEntry;
use crate::extract::reencode_entries;

#[test]
fn test_reencode_entries() {
    // Port of: TestReencodeEntries (lines 80-100)
    let entries = vec![
        DirEntry {
            tile_id: 0,
            offset: 400,
            length: 10,
            run_length: 1,
        },
        DirEntry {
            tile_id: 1,
            offset: 500,
            length: 20,
            run_length: 2,
        },
    ];

    let (reencoded, result, datalen, addressed, contents) = reencode_entries(entries);

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].src_offset, 400);
    assert_eq!(result[0].length, 10);
    assert_eq!(result[1].src_offset, 500);
    assert_eq!(result[1].length, 20);

    assert_eq!(reencoded.len(), 2);
    assert_eq!(reencoded[0].offset, 0);
    assert_eq!(reencoded[1].offset, 10);

    assert_eq!(datalen, 30);
    assert_eq!(addressed, 3);
    assert_eq!(contents, 2);
}

#[test]
fn test_reencode_entries_duplicate() {
    // Port of: TestReencodeEntriesDuplicate (lines 102-124)
    let entries = vec![
        DirEntry {
            tile_id: 0,
            offset: 400,
            length: 10,
            run_length: 1,
        },
        DirEntry {
            tile_id: 1,
            offset: 500,
            length: 20,
            run_length: 1,
        },
        DirEntry {
            tile_id: 2,
            offset: 400,
            length: 10,
            run_length: 1,
        },
    ];

    let (reencoded, result, datalen, addressed, contents) = reencode_entries(entries);

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].src_offset, 400);
    assert_eq!(result[0].length, 10);
    assert_eq!(result[1].src_offset, 500);
    assert_eq!(result[1].length, 20);

    assert_eq!(reencoded.len(), 3);
    assert_eq!(reencoded[0].offset, 0);
    assert_eq!(reencoded[1].offset, 10);
    assert_eq!(reencoded[2].offset, 0);

    assert_eq!(datalen, 30);
    assert_eq!(addressed, 3);
    assert_eq!(contents, 2);
}

#[test]
fn test_reencode_contiguous() {
    // Port of: TestReencodeContiguous (lines 126-136)
    let entries = vec![
        DirEntry {
            tile_id: 0,
            offset: 400,
            length: 10,
            run_length: 0,
        },
        DirEntry {
            tile_id: 1,
            offset: 410,
            length: 20,
            run_length: 0,
        },
    ];

    let (_, result, _, _, _) = reencode_entries(entries);

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].src_offset, 400);
    assert_eq!(result[0].length, 30);
}
