# Inventory the timeline

Type: research
Status: resolved

## Question

Which calls does the official mobile app in `reference/immich` (tag v3.3.6) make to render the timeline after login? Name the sync or list endpoints, how paging or deltas work, and the asset fields each row needs. Write the inventory to `docs/research/timeline.md` and link it from this ticket when resolving.

## Answer

The main grid is painted from local SQLite. It does not call `GET /api/assets` or a timeline-bucket route. Per-call fields are in [Timeline](../../../docs/research/timeline.md).

Password login and OAuth fire local sync, then remote sync, and open the tab shell without waiting. A stored session shows the shell first and runs those syncs in the background. Remote sync is `GET /api/server/version`, then on the first run `DELETE /api/sync/ack` for the migration cursors, then `POST /api/sync/stream`. The app acks each same-type group with `POST /api/sync/ack`. There is no page size. The cursor is the last ack per entity type. A `SyncResetV1` line clears the remote rows and the checkpoints, and the app opens the stream again.

Against v3.3.6 the body asks for the V2/V3 types, including assets, exif, stacks, albums, partners, people, faces, OCR, and memories. The grid keeps a remote row when `deletedAt` is null, visibility is timeline, the owner is the current user or a partner with `in_timeline`, and the asset is unstacked or the stack primary. Order is `createdAt` descending. Bucket date prefers `localDateTime`. The tile reads `id`, `checksum`, `type`, `thumbHash`, `isFavorite`, `duration`, `livePhotoVideoId`, `stackId`, and `isEdited`. `width` and `height` change the row only in the short layout. The grid does not wait on the websocket. Asset events enqueue another remote sync. `AssetUploadReadyV2` writes its payload locally.
