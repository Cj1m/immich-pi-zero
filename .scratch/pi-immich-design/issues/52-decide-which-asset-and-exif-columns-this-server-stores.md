# Decide which asset and exif columns this server stores

Type: grilling
Status: resolved

## Question

The timeline, download, rendition, and duplicate responses already name the asset and exif fields they read. The column list is in [Remaining to close](../../../docs/research/remaining-to-close.md). [Decide which duplicate responses this server matches](48-decide-which-duplicate-responses-this-server-matches.md) stores the group as `asset.duplicateId` and the scan time as `asset_job_status.duplicatesDetectedAt`. There is no duplicate table. v3.3.6 stores the row in Postgres. This server stores it in SQLite. Thumbnail, preview, and encoded-video paths stay as [Inventory where rendition files live](13-inventory-where-rendition-files-live.md) named them. Which asset and exif columns does this server keep?

## Comments

Recommendation accepted: keep the v3.3.6 asset and exif columns, plus the job-status scan times those jobs already write. Postgres `bytea` is a blob. `uuid` and `timestamptz` are text. A Postgres array is JSON text.

## Answer

`asset` keeps `id`, `ownerId`, `type`, `originalPath`, `originalFileName`, `fileCreatedAt`, `fileModifiedAt`, `localDateTime`, `isFavorite`, `duration`, `checksum`, `checksumAlgorithm`, `livePhotoVideoId`, `thumbhash`, `isOffline`, `libraryId`, `isExternal`, `deletedAt`, `stackId`, `duplicateId`, `status`, `updateId`, `visibility`, `width`, `height`, `isEdited`, `createdAt`, and `updatedAt`.

`id` and the other ids are text. `checksum` and `thumbhash` are blobs. The timestamps are text. `isFavorite`, `isOffline`, `isExternal`, and `isEdited` are boolean. `isFavorite` and `isEdited` default to false, and so do `isOffline` and `isExternal`. `visibility` defaults to timeline. `status` defaults to active. `duration`, `width`, `height`, `livePhotoVideoId`, `thumbhash`, `libraryId`, `deletedAt`, `stackId`, and `duplicateId` are nullable. `libraryId` stays null on this server. There is no duplicate table. `duplicateId` is the group.

`asset_exif` keeps `assetId`, `make`, `model`, `exifImageWidth`, `exifImageHeight`, `fileSizeInByte`, `orientation`, `dateTimeOriginal`, `modifyDate`, `lensModel`, `fNumber`, `focalLength`, `iso`, `latitude`, `longitude`, `city`, `state`, `country`, `description`, `fps`, `exposureTime`, `livePhotoCID`, `timeZone`, `projectionType`, `profileDescription`, `colorspace`, `bitsPerSample`, `autoStackId`, `rating`, `tags`, `updatedAt`, `updateId`, and `lockedProperties`.

`assetId` is the primary key. `description` defaults to an empty string. `tags` and `lockedProperties` are JSON text and nullable. `fileSizeInByte` is an integer. The coordinates and the other decimals are real numbers.

`asset_job_status` keeps `assetId`, `facesRecognizedAt`, `metadataExtractedAt`, `duplicatesDetectedAt`, and `ocrAt`. The times are nullable text. Face detection, metadata, duplicate detection, and OCR write their own time so a later scan can skip that Asset.

`asset_file` and `encoded_video` stay as [Inventory where rendition files live](13-inventory-where-rendition-files-live.md) named them. The CLIP vector stays the float32 blob from [Decide how this server stores the CLIP vector](26-decide-how-this-server-stores-the-clip-vector.md).
