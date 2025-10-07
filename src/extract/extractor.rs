use std::io::{BufWriter, SeekFrom};
use std::sync::Arc;

use bytes::Bytes;
use countio::Counter;
use futures_util::stream::{StreamExt, TryStreamExt};
use tokio::sync::RwLock;

use crate::async_reader::{AsyncBackend, AsyncPmTilesReader};
use crate::extract::{BoundingBox, ExtractStats, ExtractionPlan};
use crate::header::HEADER_SIZE;
use crate::{DirectoryCache, Header, PmtError, PmtResult};

/// Progress callback receiving a value between 0.0 and 1.0.
pub type ExtractProgressCallback = dyn Fn(f64) + Send + Sync;

/// Builder for extracting a subset of tiles from a `PMTiles` archive.
pub struct Extractor<'a, 'b, B, C> {
    reader: &'a AsyncPmTilesReader<B, C>,

    min_zoom: Option<u8>,
    max_zoom: Option<u8>,

    /// Overfetch ratio (0.0-1.0). Higher values trade bandwidth for fewer requests.
    overfetch: f32,

    /// Number of concurrent requests for fetching data.
    concurrency: usize,

    progress: Option<&'b ExtractProgressCallback>,
}

impl<'a, B: AsyncBackend + Sync + Send, C: DirectoryCache + Sync + Send> Extractor<'a, '_, B, C> {
    /// Creates a new extractor.
    pub fn new(reader: &'a AsyncPmTilesReader<B, C>) -> Self {
        Self {
            reader,
            min_zoom: None,
            max_zoom: None,
            overfetch: 0.05,
            concurrency: 4,
            progress: None,
        }
    }

    /// Sets the minimum zoom level to extract (inclusive).
    ///
    /// If not set, the archive's minimum zoom level will be used.
    #[must_use]
    pub fn min_zoom(mut self, min_zoom: u8) -> Self {
        self.min_zoom = Some(min_zoom);
        self
    }

    /// Sets the maximum zoom level to extract (inclusive).
    ///
    /// If not set, the archive's maximum zoom level will be used.
    #[must_use]
    pub fn max_zoom(mut self, max_zoom: u8) -> Self {
        self.max_zoom = Some(max_zoom);
        self
    }

    /// Sets the overfetch parameter for range merging (0.0 - 1.0).
    ///
    /// Higher values allow downloading more unused data to reduce the number of HTTP requests.
    /// Default is 0.05 (5%).
    #[must_use]
    pub fn overfetch(mut self, overfetch: f32) -> Self {
        self.overfetch = overfetch;
        self
    }

    /// Sets the number of concurrent requests for fetching data.
    ///
    /// Default is 4.
    #[must_use]
    pub fn concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency;
        self
    }

    /// Sets a progress callback that will be invoked periodically during extraction.
    ///
    /// The callback receives a value between 0.0 and 1.0 indicating completion progress.
    pub fn progress<'c>(self, progress: &'c ExtractProgressCallback) -> Extractor<'a, 'c, B, C> {
        Extractor {
            max_zoom: self.max_zoom,
            min_zoom: self.min_zoom,
            overfetch: self.overfetch,
            concurrency: self.concurrency,
            reader: self.reader,
            progress: Some(progress),
        }
    }

    /// Returns the header from the source `PMTiles` archive.
    #[must_use]
    pub fn input_header(&self) -> &Header {
        self.reader.get_header()
    }

    /// Port of Extract from go-pmtiles/pmtiles/extract.go:252
    ///
    /// # Errors
    ///
    /// Returns an error if extraction fails.
    pub async fn extract_bbox_to_writer<W: std::io::Write + std::io::Seek>(
        &self,
        bbox: BoundingBox,
        output: &mut W,
    ) -> PmtResult<ExtractStats> {
        let extraction_plan = self.prepare(bbox).await?;
        self.extract_to_writer(extraction_plan, output).await
    }

    /// Prepare an extraction by determining which tiles are needed.
    ///
    /// Reads the header and traverses the index, telling you which tiles you'll need
    /// and how big the extract will be.
    ///
    /// # Arguments
    ///
    /// * `bbox` - Geographic bounding box defining the region to extract
    ///
    /// # Errors
    ///
    /// Returns an error if the archive is not clustered or if reading fails.
    pub async fn prepare(&self, bbox: BoundingBox) -> PmtResult<ExtractionPlan> {
        use crate::extract::{merge_ranges, reencode_entries, relevant_entries};

        self.report_progress(0.0);
        if !self.input_header().clustered {
            return Err(PmtError::InvalidHeader);
        }

        let min_zoom = self
            .min_zoom
            .unwrap_or(self.input_header().min_zoom)
            .max(self.input_header().min_zoom);
        let max_zoom = self
            .max_zoom
            .unwrap_or(self.input_header().max_zoom)
            .min(self.input_header().max_zoom);
        if min_zoom > max_zoom {
            return Err(PmtError::InvalidHeader);
        }

        let relevance_bitmap = bbox.tile_bitmap(min_zoom, max_zoom)?;
        log::debug!("Relevant tiles: {}", relevance_bitmap.len());

        let root_entries = &self.reader.root_directory.entries;
        let (mut tile_entries, leaf_entries) =
            relevant_entries(&relevance_bitmap, max_zoom, root_entries);
        log::debug!(
            "Root directory: {} tile entries, {} leaf entries",
            tile_entries.len(),
            leaf_entries.len()
        );

        // Fetch and process leaf directories in parallel (with concurrency limit)
        let num_leaf_entries = leaf_entries.len();
        let completed_requests = Arc::new(RwLock::new(0));
        let leaf_dirs: Vec<crate::Directory> =
            futures_util::stream::iter(leaf_entries.into_iter().enumerate().map(
                |(idx, leaf_entry)| {
                    #[allow(clippy::cast_possible_truncation)]
                    let offset = (self.input_header().leaf_offset + leaf_entry.offset) as usize;
                    let length = leaf_entry.length as usize;
                    let completed_requests = completed_requests.clone();
                    async move {
                        log::debug!(
                            "Reading leaf directory {}/{}: offset={}, length={}",
                            idx + 1,
                            num_leaf_entries,
                            offset,
                            length
                        );
                        let result = self.reader.read_directory(offset, length).await;
                        let progress_complete = {
                            let mut completed_requests = completed_requests.write().await;
                            *completed_requests += 1;
                            #[allow(clippy::cast_precision_loss)]
                            {
                                f64::from(*completed_requests) / num_leaf_entries as f64
                            }
                        };
                        self.report_progress(progress_complete);
                        result
                    }
                },
            ))
            .buffered(self.concurrency)
            .try_collect()
            .await?;

        for leaf_dir in leaf_dirs {
            let (new_tiles, _new_leaves) =
                relevant_entries(&relevance_bitmap, max_zoom, &leaf_dir.entries);
            log::debug!("  Found {} relevant tiles in this leaf", new_tiles.len());
            tile_entries.extend(new_tiles);
        }

        log::debug!("Total tiles to extract: {}", tile_entries.len());

        tile_entries.sort_by_key(|e| e.tile_id);
        let (reencoded_entries, tile_ranges, tile_data_length, addressed_tiles, tile_contents) =
            reencode_entries(tile_entries);
        let (overfetch_ranges, total_tile_transfer_bytes) =
            merge_ranges(&tile_ranges, self.overfetch);

        self.report_progress(1.0);

        let num_tile_reqs = overfetch_ranges.len();
        Ok(ExtractionPlan {
            reencoded_entries,
            overfetch_ranges,
            stats: ExtractStats {
                min_zoom,
                max_zoom,
                bbox,
                total_tile_transfer_bytes,
                tile_data_length,
                addressed_tiles,
                tile_contents,
                num_leaf_entries,
                num_tile_reqs,
            },
        })
    }

    /// Port of Extract from go-pmtiles/pmtiles/extract.go:252
    ///
    /// # Errors
    ///
    /// Returns an error if writing fails or if fetching tile data fails.
    #[allow(clippy::too_many_lines)]
    pub async fn extract_to_writer<W: std::io::Write + std::io::Seek>(
        &self,
        mut plan: ExtractionPlan,
        mut output: &mut W,
    ) -> PmtResult<ExtractStats> {
        use crate::directory::MAX_ROOT_DIR_BYTES;
        use crate::writer::WriteTo;
        // Use source compression
        let compression = self.input_header().internal_compression;

        // Build new directories (optimize into root + leaves if needed)
        let reencoded_entries_len = plan.reencoded_entries.len();
        let (new_root, new_leaves) = crate::directory::optimize_directories(
            std::mem::take(&mut plan.reencoded_entries),
            MAX_ROOT_DIR_BYTES,
            compression,
        )?;

        let metadata_bytes = match self.input_header().metadata_length {
            0 => Bytes::new(),
            len => {
                #[allow(clippy::cast_possible_truncation)]
                let offset = self.input_header().metadata_offset as usize;
                #[allow(clippy::cast_possible_truncation)]
                let len = len as usize;
                self.backend().read_exact(offset, len).await?
            }
        };

        let mut new_header = self.input_header().clone();

        new_header.min_zoom = plan.min_zoom();
        new_header.max_zoom = plan.max_zoom();

        let bbox = plan.bbox();
        new_header.min_longitude = bbox.min_lon;
        new_header.min_latitude = bbox.min_lat;
        new_header.max_longitude = bbox.max_lon;
        new_header.max_latitude = bbox.max_lat;
        new_header.center_longitude = f64::midpoint(bbox.min_lon, bbox.max_lon);
        new_header.center_latitude = f64::midpoint(bbox.min_lat, bbox.max_lat);
        new_header.center_zoom = plan.min_zoom();

        new_header.internal_compression = compression;

        // Update tile counts
        new_header.n_addressed_tiles = std::num::NonZeroU64::new(plan.addressed_tiles());
        new_header.n_tile_entries = std::num::NonZeroU64::new(reencoded_entries_len as u64);
        new_header.n_tile_contents = std::num::NonZeroU64::new(plan.tile_contents());

        // Write everything preceding the tile data
        output.seek(SeekFrom::Start(HEADER_SIZE as u64))?;

        let root_length =
            new_root.write_compressed_to_counted(&mut Counter::new(&mut output), compression)?;
        new_header.root_length = root_length as u64;
        output.write_all(&metadata_bytes)?;
        let mut leaf_length = 0;
        for leaf in new_leaves {
            leaf_length += leaf.write_compressed_to_counted(
                &mut Counter::new(BufWriter::new(&mut output)),
                compression,
            )?;
        }
        new_header.leaf_length = leaf_length as u64;

        // Update offsets
        new_header.root_offset = HEADER_SIZE as u64;
        new_header.metadata_offset = new_header.root_offset + new_header.root_length;
        new_header.leaf_offset = new_header.metadata_offset + new_header.metadata_length;
        new_header.data_offset = new_header.leaf_offset + new_header.leaf_length;
        new_header.data_length = plan.tile_data_length();

        // Pop back to the beginning now that we have the header offsets
        {
            let current_pos = output.stream_position()?;
            output.rewind()?;
            new_header.write_to(&mut output)?;
            output.seek(SeekFrom::Start(current_pos))?;
        }

        // Fetch and write tile data using merged ranges
        #[allow(clippy::cast_precision_loss)]
        {
            log::debug!(
                "Fetching tile data: {} requests, {} bytes total ({} actual tiles, {:.1}% overfetch)",
                plan.overfetch_ranges().len(),
                plan.total_tile_transfer_bytes(),
                plan.tile_data_length(),
                if plan.tile_data_length() > 0 {
                    ((plan.total_tile_transfer_bytes() - plan.tile_data_length()) as f64
                        / plan.tile_data_length() as f64)
                        * 100.0
                } else {
                    0.0
                }
            );
        }

        // Fetch tile ranges in parallel (with concurrency limit)
        let total_request_count = plan.overfetch_ranges().len();
        let completed_reqs_and_bytes = Arc::new(RwLock::new((0, 0)));
        let output = Arc::new(RwLock::new(output));
        let _results: Vec<()> = futures_util::stream::iter(
            plan.overfetch_ranges()
                .to_vec()
                .into_iter()
                .enumerate()
                .map(|(idx, overfetch_range)| {
                    let data_offset = self.input_header().data_offset;
                    let completed_reqs_and_bytes = completed_reqs_and_bytes.clone();
                    let output = output.clone();
                    let total_tile_transfer_bytes = plan.total_tile_transfer_bytes();
                    async move {
                        let src_offset = try_into_usize(data_offset + overfetch_range.range.src_offset)?;
                        let length = try_into_usize(overfetch_range.range.length)?;
                        log::debug!(
                                "Request {}/{total_request_count}: offset={src_offset}, length={length} bytes",
                                idx + 1,
                            );

                        let bytes = self.backend().read_exact(src_offset, length).await?;

                        // Write the fetched data to output
                        let dst_offset = new_header.data_offset + overfetch_range.range.dst_offset;

                        let mut output = output.write().await;
                        output.seek(SeekFrom::Start(dst_offset))?;
                        // Process copy/discard instructions - write wanted bytes, skip discard bytes
                        let mut pos = 0;
                        for cd in &overfetch_range.copy_discards {
                            let wanted = try_into_usize(cd.wanted)?;
                            let discard = try_into_usize(cd.discard)?;
                            output.write_all(&bytes[pos..pos + wanted])?;
                            pos += wanted + discard;
                        }
                        drop(output);

                        #[allow(clippy::cast_precision_loss)]
                        let progress_completed = {
                            let mut completed_reqs_and_bytes = completed_reqs_and_bytes.write().await;
                            completed_reqs_and_bytes.0 += 1;
                            completed_reqs_and_bytes.1 += length;
                            let req_ratio = f64::from(completed_reqs_and_bytes.0) / total_request_count as f64;
                            let byte_ratio = completed_reqs_and_bytes.1 as f64 / total_tile_transfer_bytes as f64;
                            req_ratio * 0.3 + byte_ratio * 0.7
                        };
                        self.report_progress(progress_completed);
                        PmtResult::Ok(())
                    }
                })
        )
            .buffered(self.concurrency)
            .try_collect()
            .await?;
        Ok(plan.stats)
    }

    fn backend(&self) -> &B {
        &self.reader.backend
    }

    fn report_progress(&self, ratio_complete: f64) {
        if let Some(progress) = &self.progress {
            progress(ratio_complete);
        }
    }
}

fn try_into_usize(v: u64) -> PmtResult<usize> {
    v.try_into().map_err(PmtError::IoRangeOverflow)
}
