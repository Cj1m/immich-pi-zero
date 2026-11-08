# Immich-Compatible Rust Server for Raspberry Pi Zero 2 W

## Project Reference

Clone the official Immich monorepo for API reference only:

```bash
git clone https://github.com/immich-app/immich.git
```

Primary reference material:
- `open-api/immich-openapi-specs.json` — the API contract used by the official mobile app
- `server/src/controllers/` and `server/src/dtos/` — endpoint shapes and request/response models
- `server/src/services/` — business logic reference

Do **not** attempt to port the existing NestJS server. Build a new Rust binary that implements the same HTTP API.

## Goal

A single-binary Rust server that speaks the Immich REST API (so the official Immich mobile app works), runs comfortably on a **Raspberry Pi Zero 2 W** (512 MB RAM, quad-core Cortex-A53), stores originals in S3-compatible object storage, keeps metadata and thumbnail cache on local disk, and can ingest metadata produced by an external ML machine.

## Hardware Target

- **Board**: Raspberry Pi Zero 2 W
- **CPU**: Quad-core Cortex-A53 (ARMv8, 64-bit) @ 1 GHz
- **RAM**: 512 MB total
- **Architecture**: `aarch64-unknown-linux-gnu`
- Single concurrent user is the primary target; multi-user should still work if it remains cheap.

## Hard Constraints

- Memory budget: aim for **<150–200 MB RSS** under normal load.
- No Postgres, no Redis, no Node.js, no heavy ML models in-process.
- Videos are never transcoded. Always stream the original file.
- Thumbnails generated on demand (or shortly after upload) and cached to local disk.
- Originals live in S3; only metadata + thumbs are local.

## Architecture Decisions

- **Language**: Rust (stable).
- **HTTP**: `axum` + `tower`.
- **Database**: SQLite via `sqlx` (or `rusqlite`). WAL mode. Single file. Replaces Postgres.
- **Object storage**: S3-compatible client (`aws-sdk-s3` or `rust-s3`). Originals live in the bucket. Server never holds the full file in RAM.
- **Local storage**: One directory for SQLite + thumbnail cache + any temporary files.
- **Jobs**: Simple in-process channel or file-based queue for thumbnail generation and metadata extraction. No Redis. A small worker pool (1–2 threads) is acceptable given the Zero 2 W’s four cores.
- **Auth**: Full multi-user with sessions + API keys. First user created becomes admin. Schema should support multi-user from the start.
- **ML integration**: Accept pre-computed metadata (faces, embeddings, objects, etc.) via API or file drop from the external PC. Store it in SQLite. The Pi Zero 2 W never runs models.

## Minimum Viable Feature Set

Must work with the official Immich mobile app:

- **Auth**: login, logout, session, first-user admin setup.
- **Assets**: upload (multipart), list timeline (paginated), get single asset, delete, restore from trash.
- **Thumbnails**: generate and serve cached thumbnails (multiple sizes if the API requires them).
- **Originals**: stream original file (photos and videos) from S3 with range requests if possible.
- **Albums**: create, list, add/remove assets, update, delete.
- **Metadata**: EXIF extraction on upload (use a light library or call `exiftool` if available), store date, location, dimensions, etc.
- **Shared links**: create and resolve public share links.
- **Trash**: soft-delete with restore and permanent delete.
- **Server info**: `/api/server/ping`, version, config endpoints the app checks.

## Explicitly Deferred or Simplified

- Face recognition / CLIP / smart search on the Pi itself (metadata can be injected later from external compute).
- Video transcoding of any kind.
- Real-time websockets (polling is fine for single user).
- Complex job orchestration beyond thumbnail + metadata extraction.
- Web UI (mobile app is the target).

## Storage Layout

- **Local**: `./data/immich.db` (SQLite) + `./data/thumbnails/` (cached thumbs)
- **S3**: one bucket, objects keyed by asset ID + original filename or content hash.

## Thumbnail Strategy

- On upload or first request: generate small/medium/large thumbs using a memory-conscious library (`image` crate or `fast_image_resize` + `jpeg-decoder` / `webp`).
- Cache to disk. Serve from cache thereafter.
- Never hold more than one image in memory at a time during generation.
- The Zero 2 W’s extra cores make background thumbnail generation practical.

## S3 Streaming

- On request for original: open a streaming response from S3 and pipe it to the client.
- Support HTTP Range requests if the client asks (important for video seeking).

## External ML Hook

Provide an authenticated endpoint (or simple file watcher) that accepts JSON of the shape Immich expects for faces, embeddings, or object tags, and writes it into the SQLite tables. The external PC running the official ML container (or a custom extractor) can push results when it is online.

## Build & Deployment Target

- Cross-compile for `aarch64-unknown-linux-gnu`.
- Single static-ish binary. No Docker required on the Pi (though a minimal Dockerfile is fine for development).
- Config via environment variables or a single TOML/JSON file (S3 credentials, local data path, port, etc.).

## Success Criteria

- Official Immich mobile app can log in, upload photos/videos, view timeline, open albums, see thumbnails, and play videos by streaming the original.
- Idle RAM well under 200 MB on a Pi Zero 2 W.
- Originals live in S3; only metadata + thumbs are local.
- External machine can push ML-derived metadata that the server stores and the app can later display.

## Implementation Order Suggestion

1. Auth + server info endpoints
2. Asset upload + SQLite storage + S3 original storage
3. Timeline listing + asset retrieval
4. Thumbnail generation + caching + serving
5. Albums
6. Trash / restore
7. Shared links
8. Metadata extraction
9. External ML metadata ingestion
10. Polish and multi-user edge cases

Start with the above. Expand only as needed to keep the official mobile app functional.