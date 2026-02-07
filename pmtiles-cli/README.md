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
pmtiles show "https://protomaps.github.io/PMTiles/protomaps(vector)ODbL_firenze.pmtiles"
```
