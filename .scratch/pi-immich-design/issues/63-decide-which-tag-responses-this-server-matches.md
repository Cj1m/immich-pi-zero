# Decide which tag responses this server matches

Type: grilling
Status: resolved

## Question

The tags screen and the asset tag modal call `GET /api/tags`, `PUT /api/tags`, `PUT /api/tags/{id}`, `DELETE /api/tags/{id}`, `PUT /api/tags/assets`, `PUT /api/tags/{id}/assets`, and `DELETE /api/tags/{id}/assets`. `asset_exif.tags` is already stored. A tag change queues a sidecar write. Where the official web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do the tag calls match v3.3.6, and what sidecar file does that write produce?

## Comments

Recommendation accepted: the tag calls match v3.3.6. A tag change, and the exif change from [Decide which trash and bulk asset update responses this server matches](61-decide-which-trash-and-bulk-asset-update-responses-this-server-matches.md), write an XMP sidecar beside the Original in object storage.

## Answer

The tag calls match v3.3.6. There is no Pi difference in the HTTP response. The sidecar bytes live in object storage, beside the Original.

`tag` keeps `id`, `userId`, `value`, `createdAt`, `updatedAt`, `color`, `parentId`, and `updateId`. `value` is unique per user. A child tag's `value` is the parent value, a slash, and its name. `tag_asset` is `(assetId, tagId)`. `tag_closure` is `(id_ancestor, id_descendant)`, which is how a parent tag also matches its children on the timeline. `asset_exif.tags` stays the JSON list of tag values from [Decide which asset and exif columns this server stores](52-decide-which-asset-and-exif-columns-this-server-stores.md).

A tag document is `id`, `parentId`, `name`, `value`, `createdAt`, `updatedAt`, and `color`. `name` is the last segment of `value`. The tags page reads `id`, `name`, `value`, and `color`.

`GET /api/tags` returns 200 and the caller's tags. None is `[]`. `GET /api/tags/{id}` returns 200 and one tag. A missing tag is 400 `Not found or no tag.read access`.

`PUT /api/tags` with `{ tags }` returns 200 and the tag documents. An existing value is reused. `POST /api/tags` returns 201 and one tag. A duplicate value is 400 `A tag with that name already exists`. A missing parent is 400 `Tag not found`. `PUT /api/tags/{id}` returns 200 and the tag. Renaming rewrites `value` and refreshes `asset_exif.tags` on the assets that had it. `PATCH /api/tags/{id}` is the same update.

`DELETE /api/tags/{id}` returns 204 and an empty body. The tag and its pairs go away, and those assets refresh `asset_exif.tags`. No delete access is 400 `Not found or no tag.delete access`.

`PUT /api/tags/assets` returns 200 and `{ count }`, the number of new pairs. `PUT /api/tags/{id}/assets` and `DELETE /api/tags/{id}/assets` return 200 and one `{ id, success, error? }` per asset. A new or removed pair updates `asset_exif.tags` and queues a sidecar write.

The sidecar file is `{originalPath}.xmp` in the same object storage bucket as the Original, or the existing sidecar object when one is already stored. It is an XMP file. The written fields are the locked ones among description, the original date with its time zone, latitude, longitude, rating, and the tag list. Nothing locked skips the write. An `asset_file` row of type sidecar stores that object key. The same write is what a description, date, rating, or GPS change queues.

An unsigned call is 401. A shared-link session is 403 `Forbidden`.
