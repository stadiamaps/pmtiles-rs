// Integration tests for extract functionality
// Tests actual bbox extraction with fixtures

use std::io::Cursor;

use crate::extract::{BoundingBox, Extractor};
use crate::header::HEADER_SIZE;
use crate::{AsyncPmTilesReader, MmapBackend};

#[tokio::test]
async fn test_extract_firenze_small_bbox() {
    // Port of: TestExtract (go-pmtiles/pmtiles/extract_test.go:80)
    // Extract a small bbox from the Firenze fixture

    // Open the source file
    let backend = MmapBackend::try_from(crate::tests::VECTOR_FILE)
        .await
        .unwrap();
    let reader = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

    // Small bbox in the center of Florence
    let bbox = BoundingBox::from_nesw(43.78, 11.26, 43.77, 11.24);

    // Extract to memory
    let mut output = Cursor::new(Vec::new());
    let extractor = Extractor::new(&reader);
    let stats = extractor
        .extract_bbox_to_writer(bbox, &mut output)
        .await
        .unwrap();

    // Verify we got some tiles
    assert_eq!(stats.addressed_tiles(), 31);
    assert_eq!(stats.tile_data_length(), 1_469_320);

    // Verify the output is a valid PMTiles archive
    let output_bytes = output.into_inner();
    assert!(
        output_bytes.len() >= HEADER_SIZE,
        "Output should have header"
    );
    assert_eq!(&output_bytes[0..7], b"PMTiles", "Should have magic bytes");

    // Write to temp file to test reading it back
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path().join("extracted.pmtiles");
    std::fs::write(&temp_path, &output_bytes).unwrap();

    // Try to read the extracted archive
    let extracted_backend = MmapBackend::try_from(&temp_path).await.unwrap();
    let extracted_reader = AsyncPmTilesReader::try_from_source(extracted_backend)
        .await
        .unwrap();

    // Verify header properties
    let header = extracted_reader.get_header();
    assert!(header.clustered, "Extracted archive should be clustered");
    assert_eq!(
        stats.addressed_tiles(),
        header.n_tile_entries.unwrap().get(),
        "Plan entries should match header"
    );
}

#[tokio::test]
async fn test_extract_with_zoom_range() {
    // Test extracting with specific zoom range
    let backend = MmapBackend::try_from(crate::tests::VECTOR_FILE)
        .await
        .unwrap();
    let reader = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

    // Bbox covering most of Florence
    let bbox = BoundingBox::from_nesw(43.83, 11.33, 43.73, 11.15);

    let mut output = Cursor::new(Vec::new());
    let extractor = Extractor::new(&reader).min_zoom(10).max_zoom(12);
    let stats = extractor
        .extract_bbox_to_writer(bbox, &mut output)
        .await
        .unwrap();

    // Verify we got tiles
    assert_eq!(stats.addressed_tiles(), 10);

    let output_bytes = output.into_inner();

    // Write to temp file to read back
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path().join("extracted.pmtiles");
    std::fs::write(&temp_path, &output_bytes).unwrap();

    let extracted_backend = MmapBackend::try_from(&temp_path).await.unwrap();
    let extracted_reader = AsyncPmTilesReader::try_from_source(extracted_backend)
        .await
        .unwrap();

    let header = extracted_reader.get_header();
    assert!(header.min_zoom >= 10, "Min zoom should be at least 10");
    assert!(header.max_zoom <= 12, "Max zoom should be at most 12");
}

#[tokio::test]
async fn test_extract_overfetch_reduces_requests() {
    let bbox = BoundingBox::from_nesw(43.80, 11.28, 43.75, 11.20);

    // Extract with low overfetch
    let stats_low = {
        let backend = MmapBackend::try_from(crate::tests::VECTOR_FILE)
            .await
            .unwrap();
        let reader = AsyncPmTilesReader::try_from_source(backend).await.unwrap();

        let mut output1 = Cursor::new(Vec::new());
        let extractor = Extractor::new(&reader);
        extractor
            .overfetch(0.01)
            .extract_bbox_to_writer(bbox, &mut output1)
            .await
            .unwrap()
    };

    // Extract with high overfetch
    let stats_high = {
        // Re-open the reader for second extraction
        let backend2 = MmapBackend::try_from(crate::tests::VECTOR_FILE)
            .await
            .unwrap();
        let reader2 = AsyncPmTilesReader::try_from_source(backend2).await.unwrap();

        let mut output2 = Cursor::new(Vec::new());
        let extractor2 = Extractor::new(&reader2);
        extractor2
            .overfetch(0.05)
            .extract_bbox_to_writer(bbox, &mut output2)
            .await
            .unwrap()
    };

    // Higher overfetch may transfer more bytes...
    assert!(
        stats_high.num_tile_reqs() < stats_low.num_tile_reqs(),
        "Higher overfetch should reduce requests: low={} high={}",
        stats_low.num_tile_reqs(),
        stats_high.num_tile_reqs()
    );

    // ...but it should reduce the number of requests
    assert!(
        stats_high.total_tile_transfer_bytes() > stats_low.total_tile_transfer_bytes(),
        "Higher overfetch should increase requested bytes: low={} high={}",
        stats_low.total_tile_transfer_bytes(),
        stats_high.total_tile_transfer_bytes()
    );

    // The same number of tiles should be extracted
    assert_eq!(
        stats_low.addressed_tiles(),
        stats_high.addressed_tiles(),
        "Should extract same number of tiles"
    );
}
