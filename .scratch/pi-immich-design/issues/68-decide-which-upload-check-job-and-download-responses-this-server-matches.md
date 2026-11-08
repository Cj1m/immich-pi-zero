# Decide which upload-check, job, and download responses this server matches

Type: grilling
Status: resolved

## Question

The signed-in uploader calls `POST /api/assets/bulk-upload-check` and `GET /api/server/media-types`. The viewer can call `POST /api/assets/jobs`. Download calls `POST /api/download/info` and `POST /api/download/archive`. The shared-link download is already decided. Where the official web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do these signed-in calls match v3.3.6?

## Comments

Recommendation accepted: the upload check, the media-type list, the download info, and the zip match v3.3.6. Face, metadata, and thumbnail jobs match. `transcode-video` is accepted and writes no encoded video, which [Decide which Pi constraints the app can see](issues/08-decide-which-pi-constraints-the-app-can-see.md) already set. The shared-link download stays [Decide which shared-link actions after open this server matches](issues/39-decide-which-shared-link-actions-after-open-this-server-matches.md). The zip reads each Original from object storage.

## Answer

`POST /api/assets/bulk-upload-check` returns 200 `{results}`. Each input is `{id, checksum}`. `id` is echoed. `checksum` is a SHA-1, base64 when the string is 28 characters and hex otherwise. A checksum this user already has, including trash, is `{id, action: "reject", reason: "duplicate", assetId, isTrashed}`. `isTrashed` is true when `deletedAt` is set. Anything else is `{id, action: "accept"}`. Results follow the request order. This call does not return `unsupported-format`. Another user's checksum is not a duplicate. The phone still does not send this call.

`GET /api/server/media-types` returns 200 `{video, image, sidecar}` with no login. The values are the extension keys in `reference/immich/server/src/utils/mime-types.ts` at v3.3.6. `sidecar` is `.xmp`. The uploader uses these keys.

`POST /api/assets/jobs` returns 204. The body is `{name, assetIds}`. `refresh-faces` queues face detection for each id. `refresh-metadata` queues metadata extraction. `regenerate-thumbnail` queues the thumbnail job. `transcode-video` returns 204 and does not write an encoded video. Playback stays the Original. No update access is 400 `Not found or no asset.update access`.

`POST /api/download/info` returns 201 `{totalSize, archives}`. Each archive is `{size, assetIds}`. One of `assetIds`, `albumId`, or `userId` is required, or the call is 400 `assetIds, albumId, or userId is required`. Omitted `archiveSize` is 4 GiB. A sent `archiveSize` is the limit. The asset that crosses the limit stays in that archive, and the next asset starts another. `size` is `fileSizeInByte`, and null counts as 0. `totalSize` is the sum. A row with `deletedAt` set is left out. A `userId` download also leaves out hidden assets and needs `timeline.download`. An `albumId` download needs `album.download`. An `assetIds` download needs `asset.download`. No access is 400 `Not found or no asset.download access`, or the album or timeline permission of the same shape. A live-photo motion asset is appended after the stills. While `download.includeEmbeddedVideos` is false, the embedded Android motion file is omitted. The default is false.

`POST /api/download/archive` returns 200 and the zip. The form may send `assetIds` as one comma-separated string, `archiveName`, and `edited` as the string `true` or `false`. `Content-Disposition` is `attachment; filename*=UTF-8''` plus the encoded `archiveName` and `.zip` when `archiveName` is sent. `edited` true puts the fullsize edited file in the zip when that row exists, and the Original otherwise. The Original is read from object storage. The edited file stays the local fullsize from [Inventory where rendition files live](issues/13-inventory-where-rendition-files-live.md). The sidecar is not in the zip. A repeated filename becomes `name+count.ext`. A missing id is skipped. The same download access failure is 400.

Unsigned is 401 on the signed-in calls. `GET /api/server/media-types` is public. A shared link on the job and upload-check calls is 403 `Forbidden`. A shared link on the two download calls stays ticket 39.
