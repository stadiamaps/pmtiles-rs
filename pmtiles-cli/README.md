# pmtiles-cli

CLI tool for working with PMTiles archives.
Supports local files and HTTP URLs.

## Commands

### `pmtiles show`

Inspect archive metadata.

```bash
# Inspect local file
pmtiles show input.pmtiles

# Inspect from remote file
pmtiles show "https://protomaps.github.io/PMTiles/protomaps(vector)ODbL_firenze.pmtiles";
```

### `pmtiles extract`

Extract subsets from PMTiles archives based on a bounding box.

```bash
# Extract from local file
pmtiles extract input.pmtiles output.pmtiles --bbox=11.21,43.78,11.22,43.79

# Extract from remote file
pmtiles extract "https://protomaps.github.io/PMTiles/protomaps(vector)ODbL_firenze.pmtiles" output.pmtiles --bbox=11.21,43.78,11.22,43.79
```