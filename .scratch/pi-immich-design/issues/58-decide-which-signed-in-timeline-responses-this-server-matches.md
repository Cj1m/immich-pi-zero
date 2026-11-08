# Decide which signed-in timeline responses this server matches

Type: grilling
Status: resolved

## Question

[Inventory which signed-in photo routes add a response](57-inventory-which-signed-in-photo-routes-add-a-response.md) found that the signed-in library loads `GET /api/timeline/buckets` and `GET /api/timeline/bucket`, and the viewer loads `GET /api/assets/{id}`, `GET /api/assets/{id}/ocr`, and `GET /api/faces`. The shared-link open already matched the two timeline routes for a share session. Where the official web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do the signed-in timeline and those three asset reads match v3.3.6?

## Comments

Recommendation accepted: the signed-in timeline and the three asset reads match v3.3.6. There is no Pi difference. A partner library is included only when `inTimeline` is true, as [Decide which partner responses this server matches](47-decide-which-partner-responses-this-server-matches.md) already decided.

## Answer

The signed-in timeline and the three asset reads match v3.3.6. There is no Pi difference. A bounding box is assets whose latitude and longitude fall inside `west,south,east,north`. A bad box is 400.

`GET /api/timeline/buckets` returns 200 and `{ timeBucket, count }`. `timeBucket` is the UTC month as `YYYY-MM-DD`. `count` is how many assets are in it. No assets is `[]`. The default order is descending. The default month is `localDateTime`. `orderBy=createdAt` uses `createdAt` instead. The photos page reads `timeBucket` and `count`.

`GET /api/timeline/bucket` returns 200 and one JSON object of equal-length arrays, not a list of asset documents. The month is `timeBucket` as `YYYY-MM-DDT00:00:00.000Z`. An empty month is empty arrays. The grid reads `id`, `ownerId`, `ratio`, `isFavorite`, `visibility`, `isTrashed`, `isImage`, `thumbhash` (base64, or null), `createdAt`, `fileCreatedAt`, `localOffsetHours`, `duration`, `projectionType`, and `livePhotoVideoId`. `city` and `country` are present for a signed-in caller. `latitude` and `longitude` are present only when `withCoordinates` is true. `stack` is present only when `withStacked` is true, and each entry is `[stackId, count]` or null. That count is the stack's assets that are not deleted and have timeline visibility. `isFavorite` is true only when the asset is a favorite and the caller owns it.

With no `visibility`, the bucket keeps timeline and archive. `visibility` set keeps only that value. `withStacked` true keeps an unstacked asset and a stack primary. `isTrashed` true keeps trashed assets that are not yet hard-deleted. `albumId` keeps that album. `personId` keeps an asset with that person. `tagId` keeps that tag and its child tags. `isFavorite` true or false keeps that flag. No `albumId` defaults `userId` to the caller. Another user's timeline requires `timeline.read`. Archive requires `archive.read`. A missing album, tag, or timeline is 400 `Not found or no album.read access`, `tag.read`, `timeline.read`, or `archive.read`.

`withPartners` true adds partners who shared with the caller. On the main timeline that partner is included only when `inTimeline` is true. A person filter includes those partners even when `inTimeline` is false. A soft-deleted partner is left out. `withPartners` together with locked, archive, an omitted visibility, favorite, or trash is 400 `withPartners is only supported for non-archived, non-trashed, non-favorited, non-locked assets`. Locked visibility without an unlocked session is 401 `Elevated permission is required`. Another user's locked timeline is 400 `You may not access another user's locked timeline`.

`GET /api/assets/{id}` returns 200 and the asset document, including exif, owner, people, stack, edits, and tags. The detail panel reads those exif fields. A missing asset after the access check is 400 `Asset not found`. No read access is 400 `Not found or no asset.read access`.

`GET /api/assets/{id}/ocr` returns 200 and the OCR regions. Each region has `id`, `assetId`, `text`, `textScore`, `boxScore`, and the eight box corners. The corners follow the same edit transform v3.3.6 applies. No regions is `[]`. A missing asset is 400 `Asset not found`.

`GET /api/faces?id=` returns 200 and the visible faces. Each face has `id`, `imageWidth`, `imageHeight`, the four box edges, `sourceType`, and `person` or null. Hidden faces are left out. The boxes follow the same edit transform. No read access on either route is 400 `Not found or no asset.read access`.

An unsigned call is 401. A shared-link session on the two timeline routes and on `GET /api/assets/{id}` stays as [Decide which shared-link open responses this server matches](33-decide-which-shared-link-open-responses-this-server-matches.md) decided. Faces and OCR are 403 `Forbidden` for a shared link.
