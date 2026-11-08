# Decide which signed-in album responses this server matches

Type: grilling
Status: resolved

## Question

The phone already sends album create, update, asset add and remove, user add and remove, and album delete. The signed-in web also reads `GET /api/albums`, `GET /api/albums/{id}`, and `GET /api/albums/statistics`. Where the official web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do the album responses this web reads, and the write responses the web reads back, match v3.3.6?

## Comments

Recommendation accepted: the album calls match v3.3.6. There is no Pi difference. The list and the album page read the album document. Delete removes the album row and leaves the assets.

## Answer

The album calls match v3.3.6. There is no Pi difference.

`album` keeps `id`, `albumName` default `Untitled Album`, `createdAt`, `albumThumbnailAssetId`, `updatedAt`, `description`, `deletedAt`, `isActivityEnabled` default true, `order` default `desc`, and `updateId`. `album_user` is `(albumId, userId)` with `role` default `editor`, `createId`, `createdAt`, `updateId`, and `updatedAt`. The owner role is unique per album. `album_asset` is `(albumId, assetId)` with `createdAt`, `updatedAt`, and `updateId`. Deleting an album or an asset removes the pair.

An album document is `id`, `albumName`, `description`, `createdAt`, `updatedAt`, `albumThumbnailAssetId`, `shared`, `albumUsers`, `hasSharedLink`, `assetCount`, `lastModifiedAssetTimestamp`, `startDate`, `endDate`, `isActivityEnabled`, `order`, and `contributorCounts`. `description` is `''` when unset. `albumUsers` starts with the owner. The list page reads `albumName`, `assetCount`, and `albumUsers`. The album page also reads `shared`, `hasSharedLink`, `isActivityEnabled`, and `albumThumbnailAssetId`.

`GET /api/albums` returns 200. The albums page calls it twice, `isOwned=true` and `isShared=true`. No albums is `[]`. `assetId` limits the list to albums that contain that asset and ignores the other filters. The list leaves `sharedLinks` off the document.

`GET /api/albums/{id}` returns 200 and the document, including `contributorCounts` when the album has another user or a shared link. A missing album is 400 `Not found or no album.read access`. A shared-link session can read this route, as [Decide which shared-link open responses this server matches](33-decide-which-shared-link-open-responses-this-server-matches.md) decided.

`GET /api/albums/statistics` returns 200 and `{ owned, shared, notShared }`. The usage page reads those counts.

`POST /api/albums` returns 201 and the document. The caller is the owner. The first asset becomes the thumbnail. An unknown user is 400 `Invalid user`. `PATCH /api/albums/{id}` returns 200 and the document. A thumbnail that is not in the album is 400 `Invalid album thumbnail`. No update access is 400 `Not found or no album.update access`.

`PUT /api/albums/{id}/assets` and `DELETE /api/albums/{id}/assets` return 200 and one `{ id, success, error? }` per id. `error` is `duplicate`, `no_permission`, or `not_found`. `PUT /api/albums/assets` returns 200 and `{ success, error? }`. The first new asset becomes the thumbnail when the album has none.

`PUT /api/albums/{id}/users` returns 200 and the document. Another owner is 400 `Cannot add another owner`. An unknown user is 400 `Invalid user`. `PUT /api/albums/{id}/user/{userId}` returns 204. The owner is 400 `User is owner`. `DELETE /api/albums/{id}/user/{userId}` returns 204. `me` is the caller. A user who is not on the album is 400 `Album not shared with user`. The last owner is 400 `Cannot remove the last album owner`.

`DELETE /api/albums/{id}` returns 204 and an empty body. The album row is removed. The assets stay. No delete access is 400 `Not found or no album.delete access`.

The next sync sends `AlbumV1` or `AlbumV2`, `AlbumUserV1`, and `AlbumToAssetV1`, and `AlbumDeleteV1`, `AlbumUserDeleteV1`, and `AlbumToAssetDeleteV1` when a row goes away. An unsigned call is 401. A shared-link session on a route other than the album read and its map markers is 403 `Forbidden`.
