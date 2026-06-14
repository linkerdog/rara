# Vector schema metadata

RARA now records the canonical memory vector schema next to each LanceDB vector
store.  The sidecar file uses `<lancedb-uri>.schema.json` and stores a store
schema version plus per-table vector dimensions and vector schema versions.

The write path remains the source of truth for migrations.  On upsert,
`VectorDB` validates both the physical LanceDB table schema and the sidecar
metadata.  Missing sidecar metadata is backfilled for existing valid tables.
Dimension or schema-version mismatches drop and recreate the affected table,
then write the updated sidecar metadata.

This keeps empty or migrated stores from silently inheriting stale embedding
dimensions while preserving the existing behavior that searches do not create
tables with guessed dimensions.
