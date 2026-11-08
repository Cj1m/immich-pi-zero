# Inventory where rendition files live

Type: research
Status: resolved

## Question

Where does the v3.3.6 server in `reference/immich` store the thumbnail and preview files that `GET /api/assets/{id}/thumbnail` reads, and the encoded video that `GET /api/assets/{id}/video/playback` prefers over the Original? Name the directories, the `asset_file` types, and the job that writes each one. [Inventory thumbnails and originals](04-inventory-thumbnails-and-originals.md) records the requests. This ticket is where those bytes live on disk. Write the note to `docs/research/rendition-files.md` and link it from this ticket when resolving.

## Answer

Thumbnail and preview are `asset_file` rows under the media location's `thumbs` directory. The encoded video is an `encoded_video` row under `encoded-video`. Directories, types, and jobs are in [Rendition files](../../../docs/research/rendition-files.md).

The media location is `IMMICH_MEDIA_LOCATION` when that absolute path is set. Otherwise the server uses `/data` or `/usr/src/app/upload` when exactly one of those directories exists, and `/usr/src/app/upload` otherwise. `asset_file.path` stores the absolute path, and `sendFile` uses it as stored.

A thumbnail has type `thumbnail`. The default path is `{mediaLocation}/thumbs/{ownerId}/{id[0:2]}/{id[2:4]}/{id}_thumbnail.webp`. A preview has type `preview`, in that same directory, default `{id}_preview.jpeg`. An edited copy is the same type with `isEdited` true and `_edited` before the extension. `GET /api/assets/{id}/thumbnail` opens the edited row when `edited=true` and that row exists, and the other row of the same type when it does not. A missing row for that size is 404.

`AssetGenerateThumbnails` writes both files. Upload reaches it after metadata extraction and the storage-template job. A still is sharp, written to the final path. A video, or a filename ending in `.gif`, is one ffmpeg frame written to the same paths. `AssetEditThumbnailGeneration` writes or removes the edited pair when an edit is saved or cleared. The thumbhash is the `asset.thumbhash` column.

The encoded file is `{mediaLocation}/encoded-video/{ownerId}/{id[0:2]}/{id[2:4]}/{id}.mp4`, type `encoded_video`, `isEdited` false. `AssetEncodeVideo` writes it when the transcode policy requires a new file. The default policy is `required`. `GET /api/assets/{id}/video/playback` opens that path when the row exists, and `asset.originalPath` when the row is absent. A chosen path that is missing on disk is 404.
