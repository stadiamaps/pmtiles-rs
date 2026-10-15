//! How the stream writer finds tiles with identical content.

use std::hash::Hash;
use std::marker::PhantomData;

mod sealed {
    pub trait Sealed {}
}

/// How a [`PmTilesStreamWriter`](crate::PmTilesStreamWriter) finds tiles with identical content,
/// so that it stores them once.
///
/// It is chosen once, on the [`PmTilesWriter`](crate::PmTilesWriter), and it decides the signature
/// of `add_tile`, so a writer has exactly one way to add tiles:
///
/// - [`ContentHash`] (the default): `add_tile(coord, data)`, and the writer hashes every tile.
/// - [`CallerKey`], chosen with [`dedup_by_key`](crate::PmTilesWriter::dedup_by_key):
///   `add_tile(coord, data, key)`, and the caller says which tiles repeat.
///
/// ```
/// # use pmtiles::{PmTilesWriter, TileCoord, TileType};
/// # let coord = TileCoord::new(0, 0, 0).unwrap();
/// let out = std::io::Cursor::new(Vec::new());
/// let mut writer = PmTilesWriter::new(TileType::Mvt).create(out).unwrap();
/// writer.add_tile(coord, b"tile").unwrap();
///
/// let out = std::io::Cursor::new(Vec::new());
/// let mut writer = PmTilesWriter::new(TileType::Mvt).dedup_by_key::<u64>().create(out).unwrap();
/// writer.add_tile(coord, b"ocean", Some(1)).unwrap();
/// writer.add_tile(coord, b"unique", None).unwrap();
/// ```
///
/// A writer that takes keys cannot be given a tile without one:
///
/// ```compile_fail
/// # use pmtiles::{PmTilesWriter, TileCoord, TileType};
/// # let coord = TileCoord::new(0, 0, 0).unwrap();
/// let out = std::io::Cursor::new(Vec::new());
/// let mut writer = PmTilesWriter::new(TileType::Mvt).dedup_by_key::<u64>().create(out).unwrap();
/// writer.add_tile(coord, b"tile").unwrap();
/// ```
pub trait DedupMode: sealed::Sealed {
    /// What identifies stored content that may be reused.
    type Key: Copy + Eq + Hash;
}

/// Deduplicate by a 128-bit `XxHash3` of each tile's bytes.
///
/// The bytes themselves are not compared, but a collision is negligibly unlikely even for billions
/// of tiles. The writer keeps one hash per distinct tile in memory.
#[derive(Debug)]
pub struct ContentHash(());

impl sealed::Sealed for ContentHash {}

impl DedupMode for ContentHash {
    type Key = u128;
}

/// Deduplicate by keys the caller provides, e.g. a generator that knows which tiles are all ocean.
///
/// Nothing is hashed. Tiles given the same key must have identical bytes: they are stored once,
/// and consecutive ones as a single run. A tile given no key is stored as is and not remembered,
/// so the writer keeps in memory only the contents the caller expects to repeat.
#[derive(Debug)]
pub struct CallerKey<K = u64>(PhantomData<fn() -> K>);

impl<K> sealed::Sealed for CallerKey<K> {}

impl<K: Copy + Eq + Hash> DedupMode for CallerKey<K> {
    type Key = K;
}
