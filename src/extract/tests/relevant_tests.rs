// Tests ported from go-pmtiles/pmtiles/extract_test.go
// TestRelevantEntries* functions (lines 9-78)

use roaring::RoaringTreemap;

use crate::directory::DirEntry;
use crate::extract::relevant_entries;

#[test]
fn test_relevant_entries() {
    // Port of: TestRelevantEntries (lines 9-20)
    let entries = vec![DirEntry {
        tile_id: 0,
        offset: 0,
        length: 0,
        run_length: 1,
    }];

    let mut bitmap = RoaringTreemap::new();
    bitmap.insert(0);

    let (tiles, leaves) = relevant_entries(&bitmap, 4, &entries);

    assert_eq!(tiles.len(), 1);
    assert_eq!(leaves.len(), 0);
}

#[test]
fn test_relevant_entries_run_length() {
    // Port of: TestRelevantEntriesRunLength (lines 22-37)
    let entries = vec![DirEntry {
        tile_id: 0,
        offset: 0,
        length: 0,
        run_length: 5,
    }];

    let mut bitmap = RoaringTreemap::new();
    bitmap.insert(1);
    bitmap.insert(2);
    bitmap.insert(4);

    let (tiles, leaves) = relevant_entries(&bitmap, 4, &entries);

    assert_eq!(tiles.len(), 2);
    assert_eq!(tiles[0].run_length, 2);
    assert_eq!(tiles[1].run_length, 1);
    assert_eq!(leaves.len(), 0);
}

#[test]
fn test_relevant_entries_leaf() {
    // Port of: TestRelevantEntriesLeaf (lines 39-50)
    let entries = vec![DirEntry {
        tile_id: 0,
        offset: 0,
        length: 0,
        run_length: 0,
    }];

    let mut bitmap = RoaringTreemap::new();
    bitmap.insert(1);

    let (tiles, leaves) = relevant_entries(&bitmap, 4, &entries);

    assert_eq!(tiles.len(), 0);
    assert_eq!(leaves.len(), 1);
}

#[test]
fn test_relevant_entries_not_leaf() {
    // Port of: TestRelevantEntriesNotLeaf (lines 52-65)
    let entries = vec![
        DirEntry {
            tile_id: 0,
            offset: 0,
            length: 0,
            run_length: 0,
        },
        DirEntry {
            tile_id: 2,
            offset: 0,
            length: 0,
            run_length: 1,
        },
        DirEntry {
            tile_id: 4,
            offset: 0,
            length: 0,
            run_length: 0,
        },
    ];

    let mut bitmap = RoaringTreemap::new();
    bitmap.insert(3);

    let (tiles, leaves) = relevant_entries(&bitmap, 4, &entries);

    assert_eq!(tiles.len(), 0);
    assert_eq!(leaves.len(), 0);
}

#[test]
fn test_relevant_entries_max_zoom() {
    // Port of: TestRelevantEntriesMaxZoom (lines 67-78)
    let entries = vec![DirEntry {
        tile_id: 0,
        offset: 0,
        length: 0,
        run_length: 0,
    }];

    let mut bitmap = RoaringTreemap::new();
    bitmap.insert(6);

    let (_, leaves) = relevant_entries(&bitmap, 1, &entries);
    assert_eq!(leaves.len(), 0);

    let (_, leaves) = relevant_entries(&bitmap, 2, &entries);
    assert_eq!(leaves.len(), 1);
}
