# Decide which shared-link actions after open this server matches

Type: grilling
Status: resolved

## Question

[Inventory what the web UI does after a shared link is open](38-inventory-what-the-web-ui-does-after-a-shared-link-is-open.md) names the calls after first paint. The note is [Shared link after open](../../../docs/research/shared-link-after-open.md). Where the Web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Which of those calls does this server answer the way v3.3.6 does, and which response changes because the Pi cannot?

Leave the activity routes as [Decide which activity fields this server stores](34-decide-which-activity-fields-this-server-stores.md) left them. The shared-link pages do not call them. A shared-link session gets 403 `Forbidden` on every activity route, including `GET /api/activities/statistics`. Playback of a shared video stays `GET /api/assets/{id}/video/playback` of the Original, status 200 or 206. `realtimeTranscoding` stays false.

`GET /api/server/media-types` fires at app init. Upload reads `image` and `video` from it. `DELETE /api/shared-links/{id}/assets` is the owner remove on an individual link. Cast makes no Immich call.

## Comments

Recommendation accepted for every call after the open. The Pi changes none of them.

## Answer

This server answers every call in [Shared link after open](../../../docs/research/shared-link-after-open.md) the way v3.3.6 does. The Pi changes none of them.

A `key` or `slug` on the request is the shared link, including when the access-token cookie is also present. The user on that auth object is the link's owner. Download, upload, asset, thumbnail, original, and timeline calls on these pages send that query. `DELETE /api/shared-links/{id}/assets` does not, so it is the signed-in User's Session.

Playback of a shared video stays the Original, status 200 or 206, and `realtimeTranscoding` stays false, as [Decide which shared-link open responses this server matches](33-decide-which-shared-link-open-responses-this-server-matches.md) decided. Hover, the opened viewer, a panorama video, and a live photo's motion playback are that same route. They do not request HLS. Activity routes stay as [Decide which activity fields this server stores](34-decide-which-activity-fields-this-server-stores.md) left them. These pages do not call them.

`GET /api/assets/{id}`, `GET /api/timeline/bucket`, and the thumbnail grid stay that open. Opening one Asset, the next and previous Assets, a slideshow month that is not loaded, and a one-Asset download's info lookup reuse those responses. `showMetadata` false still strips the asset. A shared link still sends no `owner`, and `people` is still `[]`.

`GET /api/assets/{id}/thumbnail` with the share query is 200 and the image. The shared link forces `edited` true. `size=thumbnail` and `size=preview` open the edited row when it exists, and the other row of that size when it does not. A missing row is 404. `Cache-Control` is `private, max-age=86400, no-transform, stale-while-revalidate=2592000, stale-if-error=2592000`. A `Range` header is 206. `size=fullsize` with no row redirects 302 to `size=preview`. `allowDownload` false still serves the thumbnail. The view permission is `asset.view`.

`GET /api/assets/{id}/original` is the file. The shared link forces `edited` true. An edited still opens the fullsize edited file when that row exists, and the Original when it does not. The thumbnail job writes that file for a still that has an Edit, at `{id}_fullsize_edited.jpeg` under the same thumbs path [Inventory where rendition files live](13-inventory-where-rendition-files-live.md) names. A video has no such row, so the body is the Original. Status is 200, or 206 when the client sends `Range`. The same cache header is set. `Content-Disposition` is `inline`, and the filename keeps `originalFileName` with the opened file's extension. `allowDownload` false is 400 `{ "message": "Not found or no asset.download access" }`. A missing file is 404.

`POST /api/download/info` is 201 `{ "totalSize", "archives" }`. Each archive is `{ "size", "assetIds" }`. An album download sends `albumId`. The whole link and a multi-select send `assetIds`. A recipient sends no `archiveSize`, and the limit is 4 GiB (4 × 1024³). A signed-in User on the page sends their own `archiveSize`, and that number is the limit. The asset that crosses the limit stays in the archive that crossed it. The next asset starts another archive. `size` is the exif file size, and a null size counts as 0. A trashed Asset is left out. `totalSize` is the sum. No `assetIds`, `albumId`, or `userId` is 400 `{ "message": "assetIds, albumId, or userId is required" }`. `allowDownload` false is 400 `{ "message": "Not found or no asset.download access" }` or `{ "message": "Not found or no album.download access" }`. An empty `archives` array is still 201, and the page saves nothing.

A live-photo motion Asset is appended after the stills. The link owner's `download.includeEmbeddedVideos` defaults to false. While it is false, the info list omits the embedded Android motion Original, the hidden Asset v3.3.6 stores as `{motionId}-MP.mp4` under `encoded-video`. This server stores that Original in object storage and still omits it. A motion Asset the phone uploaded on its own stays in the list. The archive call allows a motion id that belongs to a still on the link.

`POST /api/download/archive` is 200 and the zip. The form sends `assetIds` as one comma-separated string, `archiveName`, and `edited` as the string `true`. `Content-Disposition` is `attachment; filename*=UTF-8''` plus the encoded `archiveName` and `.zip`. `edited` true puts the fullsize edited file in the zip when that row exists, and the Original otherwise. A repeated filename becomes `name+count.ext`. An id that is not an Asset is skipped. The same `allowDownload` failure is 400.

`POST /api/assets` with the share query is 201 `{ "status": "created", "id" }` for a new Asset, and 200 `{ "status": "duplicate", "id" }` when the SHA-1 of the bytes already belongs to the link owner, including a trashed Asset. The server hashes the file. The page sends no checksum. The owner is the link's User. The bytes are that User's Original. A duplicate id is attached as well. An individual link gains the id. An album link adds the id to the album. The page does not refetch. A later timeline load includes the new id. `allowUpload` false is 401 `{ "message": "Unauthorized" }`, which is what a drop does when the button is hidden. Quota is 400 `{ "message": "Quota has been exceeded!" }`. An empty file is 400 `{ "message": "File is empty" }`. A type outside the media-type lists is 400 `{ "message": "Unsupported file type ..." }`.

`GET /api/server/media-types` is public and 200 `{ "image", "video", "sidecar" }`. `image`, `video`, and `sidecar` are the extension keys v3.3.6 uses for those three maps. Upload accepts a file whose extension is in `image` or `video`.

`DELETE /api/shared-links/{id}/assets` with `{ "assetIds" }` is 200 and one `{ "assetId", "success", "error" }` per id. The signed-in owner of an individual link gets `success` true, and the page drops that id. An id that was not on the link is `success` false and `error` `not_found`. An album link is 400 `{ "message": "Invalid shared link type" }`. A missing link, or a link owned by someone else, is 400 `{ "message": "Shared link not found" }`. No Session is 401 `{ "message": "Authentication required" }`. A shared-link session on this route is 403 `Forbidden`.

The map modal reads `map.darkStyle` and `map.lightStyle` from the config already loaded on the open. The defaults are `https://tiles.immich.cloud/v1/style/dark.json` and `https://tiles.immich.cloud/v1/style/light.json`. The browser requests those URLs. This server does not proxy them. Cast makes no Immich call.
