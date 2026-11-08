# Decide which trash and bulk asset update responses this server matches

Type: grilling
Status: resolved

## Question

The phone and the web send `POST /api/trash/empty`, `POST /api/trash/restore`, and `POST /api/trash/restore/assets`. The web also sends `PUT /api/assets` for a date, a description, a rating, a GPS point, or visibility. Favorite and clearing `duplicateId` are already decided. Where the official mobile app or the official web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do the trash calls and those bulk updates match v3.3.6?

## Comments

Recommendation accepted: trash and the bulk asset update match v3.3.6. Emptying trash deletes the Original from object storage. An exif change queues the sidecar write [Decide which tag responses this server matches](63-decide-which-tag-responses-this-server-matches.md) decides.

## Answer

The trash calls and the bulk update match v3.3.6. The Original lives in object storage, so emptying trash removes that object. The response the page reads does not change.

`POST /api/trash/restore/assets` with `{ ids }` returns 200 and `{ count }`. The trash action reads `count`. An empty `ids` is `{ count: 0 }` and changes nothing. Otherwise each id must be deletable, or the response is 400 `Not found or no asset.delete access`. `count` is `ids.length`. Each trashed asset returns to `active` and `deletedAt` becomes null. The next sync sends the asset with `deletedAt` null.

`POST /api/trash/restore` returns 200 and `{ count }`, the number of the caller's trashed assets restored the same way. None is `{ count: 0 }`.

`POST /api/trash/empty` returns 200 and `{ count }`, the number moved from trashed to deleted. The page reads `count`. The delete job then removes each asset, its thumbnail and preview files, and its Original from object storage. The next sync sends `AssetDeleteV1` as `{ assetId }`. A stack of one remaining asset is dissolved. Deleting the primary promotes another asset. A live-photo motion with no other still is deleted too. Quota usage drops by the file size.

`PUT /api/assets` returns 204 and an empty body. The web sends it for a date, a description, a rating, a GPS point, visibility, or `duplicateId`. `PATCH /api/assets` is the same update. No update access is 400 `Not found or no asset.update access`. Favorite and clearing `duplicateId` stay on their tickets. Setting visibility to locked removes those assets from every album. A description, date, rating, or GPS point queues a sidecar write. The file that write produces is decided with the tag calls.

`PUT /api/assets/{id}` returns 200 and the asset document when the web updates one asset, including a rating. That is the same route [Decide which favorite responses this server matches](43-decide-which-favorite-responses-this-server-matches.md) already matched for the heart.

An unsigned call is 401. A shared-link session is 403 `Forbidden`.
