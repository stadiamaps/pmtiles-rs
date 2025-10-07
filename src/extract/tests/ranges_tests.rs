// Tests ported from go-pmtiles/pmtiles/extract_test.go
// TestMergeRanges* functions (lines 138-175)

use crate::extract::ranges::{SrcDstRange, merge_ranges};

#[test]
fn test_merge_ranges() {
    // Port of: TestMergeRanges (lines 138-152)
    let ranges = vec![
        SrcDstRange {
            src_offset: 0,
            dst_offset: 0,
            length: 50,
        },
        SrcDstRange {
            src_offset: 60,
            dst_offset: 60,
            length: 60,
        },
    ];

    let (result, total_transfer_bytes) = merge_ranges(&ranges, 0.1);

    assert_eq!(result.len(), 1);
    assert_eq!(total_transfer_bytes, 120);

    let front = &result[0];
    assert_eq!(front.range.src_offset, 0);
    assert_eq!(front.range.dst_offset, 0);
    assert_eq!(front.range.length, 120);
    assert_eq!(front.copy_discards.len(), 2);
    assert_eq!(front.copy_discards[0].wanted, 50);
    assert_eq!(front.copy_discards[0].discard, 10);
    assert_eq!(front.copy_discards[1].wanted, 60);
    assert_eq!(front.copy_discards[1].discard, 0);
}

#[test]
fn test_merge_ranges_multiple() {
    // Port of: TestMergeRangesMultiple (lines 154-166)
    let ranges = vec![
        SrcDstRange {
            src_offset: 0,
            dst_offset: 0,
            length: 50,
        },
        SrcDstRange {
            src_offset: 60,
            dst_offset: 60,
            length: 10,
        },
        SrcDstRange {
            src_offset: 80,
            dst_offset: 80,
            length: 10,
        },
    ];

    let (result, total_transfer_bytes) = merge_ranges(&ranges, 0.3);

    assert_eq!(total_transfer_bytes, 90);
    assert_eq!(result.len(), 1);

    let front = &result[0];
    assert_eq!(front.range.src_offset, 0);
    assert_eq!(front.range.dst_offset, 0);
    assert_eq!(front.range.length, 90);
    assert_eq!(front.copy_discards.len(), 3);
}

#[test]
fn test_merge_ranges_non_src_ordered() {
    // Port of: TestMergeRangesNonSrcOrdered (lines 168-175)
    // Ranges not ordered by source offset should not merge
    let ranges = vec![
        SrcDstRange {
            src_offset: 20,
            dst_offset: 0,
            length: 50,
        },
        SrcDstRange {
            src_offset: 0,
            dst_offset: 60,
            length: 50,
        },
    ];

    let (result, _) = merge_ranges(&ranges, 0.1);
    assert_eq!(result.len(), 2);
}
