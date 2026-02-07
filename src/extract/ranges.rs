// Port of MergeRanges from go-pmtiles/pmtiles/extract.go:159

/// A range of bytes to copy from source to destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SrcDstRange {
    /// Offset in the source file
    pub src_offset: u64,
    /// Offset in the destination file
    pub dst_offset: u64,
    /// Number of bytes to copy
    pub length: u64,
}

impl SrcDstRange {
    pub(crate) fn src_end(&self) -> u64 {
        self.src_offset + self.length
    }
}

/// Instructions for copying data with discards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyDiscard {
    /// Number of bytes to keep
    pub wanted: u64,
    /// Number of bytes to skip
    pub discard: u64,
}

/// A merged range that may include overfetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverfetchRange {
    /// The byte range to fetch
    pub range: SrcDstRange,
    /// Instructions for which bytes to copy and which to discard
    pub copy_discards: Vec<CopyDiscard>,
}

#[derive(Debug, Clone)]
struct MergeItem {
    range: SrcDstRange,
    bytes_to_next: u64,
    copy_discards: Vec<CopyDiscard>,
    prev: Option<usize>,
    next: Option<usize>,
}

/// Merges ranges to minimize requests, trading bandwidth for fewer fetches.
///
/// Returns (`merged_ranges`, `total_bytes_transferred`).
#[must_use]
pub fn merge_ranges(ranges: &[SrcDstRange], overfetch: f32) -> (Vec<OverfetchRange>, u64) {
    if ranges.is_empty() {
        return (vec![], 0);
    }

    let mut total_size = 0_u64;
    let mut items: Vec<MergeItem> = Vec::with_capacity(ranges.len());

    // Create merge items with distance to next range
    for (i, rng) in ranges.iter().enumerate() {
        let bytes_to_next = if i == ranges.len() - 1 {
            u64::MAX
        } else {
            let gap = ranges[i + 1]
                .src_offset
                .saturating_sub(rng.src_offset + rng.length);
            if gap > 0 {
                gap
            } else {
                // Ranges overlap or are not properly ordered by source - can't merge
                u64::MAX
            }
        };

        items.push(MergeItem {
            range: *rng,
            bytes_to_next,
            copy_discards: vec![CopyDiscard {
                wanted: rng.length,
                discard: 0,
            }],
            prev: if i > 0 { Some(i - 1) } else { None },
            next: if i < ranges.len() - 1 {
                Some(i + 1)
            } else {
                None
            },
        });

        total_size += rng.length;
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let mut overfetch_budget = (total_size as f32 * overfetch) as i64;

    // Create indices sorted by bytes_to_next (ascending)
    let mut sorted_indices: Vec<usize> = (0..items.len()).collect();
    sorted_indices.sort_by_key(|&i| items[i].bytes_to_next);

    // Merge ranges while we have budget and multiple ranges
    while sorted_indices.len() > 1 {
        let idx = sorted_indices[0];
        let item_bytes_to_next = items[idx].bytes_to_next;

        // Can't merge if gap is too large (u64::MAX means can't merge)
        if item_bytes_to_next == u64::MAX {
            break;
        }

        #[expect(clippy::cast_possible_wrap)]
        if overfetch_budget - (item_bytes_to_next as i64) < 0 {
            break;
        }

        // Get the item to merge
        let Some(next_idx) = items[idx].next else {
            break; // No next item to merge into
        };

        // Merge current item into next item
        let new_length =
            items[idx].range.length + item_bytes_to_next + items[next_idx].range.length;
        items[next_idx].range = SrcDstRange {
            src_offset: items[idx].range.src_offset,
            dst_offset: items[idx].range.dst_offset,
            length: new_length,
        };

        // Update prev pointer of next item
        items[next_idx].prev = items[idx].prev;

        // Update next pointer of previous item (if exists)
        if let Some(prev_idx) = items[idx].prev {
            items[prev_idx].next = Some(next_idx);
        }

        // Update copy_discards: set discard on last element of current item
        if let Some(last) = items[idx].copy_discards.last_mut() {
            last.discard = item_bytes_to_next;
        }

        // Prepend current item's copy_discards to next item's
        let mut new_copy_discards = items[idx].copy_discards.clone();
        new_copy_discards.append(&mut items[next_idx].copy_discards);
        items[next_idx].copy_discards = new_copy_discards;

        // Remove merged item from sorted list
        sorted_indices.remove(0);

        #[expect(clippy::cast_possible_wrap)]
        {
            overfetch_budget -= item_bytes_to_next as i64;
        }

        // Re-sort (items have changed, need to re-sort)
        sorted_indices.sort_by_key(|&i| items[i].bytes_to_next);
    }

    // Sort remaining items by descending length
    sorted_indices.sort_by(|&a, &b| items[b].range.length.cmp(&items[a].range.length));

    let mut total_bytes_transferred = 0_u64;
    let merged_ranges: Vec<OverfetchRange> = sorted_indices
        .into_iter()
        .map(|i| {
            total_bytes_transferred += items[i].range.length;
            OverfetchRange {
                range: items[i].range,
                copy_discards: items[i].copy_discards.clone(),
            }
        })
        .collect();

    (merged_ranges, total_bytes_transferred)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_ranges() {
        // Port of TestMergeRanges extract_test.go#L137
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
        assert_eq!(
            front.range,
            SrcDstRange {
                src_offset: 0,
                dst_offset: 0,
                length: 120
            }
        );
        assert_eq!(front.copy_discards.len(), 2);
        assert_eq!(
            front.copy_discards[0],
            CopyDiscard {
                wanted: 50,
                discard: 10
            }
        );
        assert_eq!(
            front.copy_discards[1],
            CopyDiscard {
                wanted: 60,
                discard: 0
            }
        );
    }

    #[test]
    fn test_merge_ranges_multiple() {
        // Port of TestMergeRangesMultiple extract_test.go#L154
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
        assert_eq!(
            front.range,
            SrcDstRange {
                src_offset: 0,
                dst_offset: 0,
                length: 90
            }
        );
        assert_eq!(front.copy_discards.len(), 3);
    }

    #[test]
    fn test_merge_ranges_non_src_ordered() {
        // Port of TestMergeRangesMultiple extract_test.go#L168
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
}
