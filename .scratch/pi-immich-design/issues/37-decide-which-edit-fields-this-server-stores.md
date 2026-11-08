# Decide which edit fields this server stores

Type: grilling
Status: resolved

## Question

[Inventory which edit fields the app stores](30-inventory-which-edit-fields-the-app-stores.md) names the calls. The app stores `id`, `assetId`, `action`, `parameters`, and `sequence` from `AssetEditV1`. `parameters` is the crop `x`, `y`, `width`, and `height`, the rotate `angle`, or the mirror `axis`. `AssetEditDeleteV1` is `editId`. The asset row's `isEdited` comes from `AssetV2`. Save is `PUT /api/assets/{id}/edits` or, when the list is empty, `DELETE /api/assets/{id}/edits`. The edited thumbnail and preview paths stay the ones [Inventory where rendition files live](13-inventory-where-rendition-files-live.md) names. Where the mobile app can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Which of these fields does this server keep, and which does each response include?

## Comments

Recommendation accepted: one row per action. The columns are `id`, `assetId`, `action`, `parameters`, `sequence`, `updatedAt`, and `updateId`. `(assetId, sequence)` is unique. `sequence` is the index in the save list, starting at 0. `parameters` is one JSON object. Crop is `x`, `y`, `width`, and `height`. Rotate is `angle`. Mirror is `axis`. `updatedAt` and `updateId` stay on the row. The sync line is the five fields the app stores.

Recommendation accepted: a removed row writes an audit row of `id`, `editId`, `assetId`, and `deletedAt`. `AssetEditDeleteV1` sends `{ editId }`. The ack cursor is the audit row's `id`. Rows older than 31 days are deleted. Replacing the list audits every previous id, then inserts new ids. Deleting the Asset removes its edit rows and writes no audit row. Trashing the Asset leaves the rows.

Recommendation accepted: `isEdited` is true while any edit row exists for that Asset, and false when the last row is gone. The write that inserts or deletes the rows sets it, before `AssetEditThumbnailGeneration` is queued.

Recommendation accepted: `PUT /api/assets/{id}/edits` returns 200 `{ assetId, edits }`. Each item is `id`, `action`, and `parameters`, in `sequence` order. The list replaces every edit on that Asset. A body the v3.3.6 rules reject is 400 and stores nothing. `DELETE /api/assets/{id}/edits` is an empty 204, including when the Asset has no edits, and it queues the same job. `GET /api/assets/{id}/edits` returns that same 200 body. The mobile app does not call GET.

Recommendation accepted: PUT and DELETE succeed for the owner. GET succeeds for the owner and for a User who can already read the Asset through an album. A shared-link session receives 403 `Forbidden` on all three routes. A locked Asset fails the owner check. This server has no session elevation that lifts it.

Recommendation accepted: `AssetEditsV1` sends `AssetEditDeleteV1` first, then `AssetEditV1`. The upsert ack cursor is `updateId`. After `AssetEditThumbnailGeneration` finishes, the owner receives `AssetEditReadyV2`.

## Answer

This server keeps one edit row per crop, rotate, or mirror. The columns are `id`, `assetId`, `action`, `parameters`, `sequence`, `updatedAt`, and `updateId`. `(assetId, sequence)` is unique. `sequence` is the index in the save list, starting at 0. `id` is generated. `parameters` is one JSON object: crop is `x`, `y`, `width`, and `height`; rotate is `angle`; mirror is `axis`. `updatedAt` and `updateId` stay on the row. `AssetEditV1` sends `id`, `assetId`, `action`, `parameters`, and `sequence`.

A removed row writes an audit row: `id`, `editId`, `assetId`, and `deletedAt`. `AssetEditDeleteV1` sends `{ editId }`. The ack cursor is the audit row's `id`. Audit rows older than 31 days are deleted. `PUT` deletes every current row for that Asset, audits each previous id, then inserts the new list with new ids. Deleting the Asset removes its edit rows and writes no audit row. The asset delete is enough for the phone, which drops the edit rows with the Asset. Trashing the Asset leaves the rows and leaves `isEdited` as it was. Sync still sends them to the owner.

`isEdited` on the Asset is true while any edit row exists for it, and false when the last row is gone. The write that inserts or deletes the rows sets it, before `AssetEditThumbnailGeneration` is queued. The viewer can still receive the unedited file until that job writes the edited one. The edited paths stay the ones [Inventory where rendition files live](13-inventory-where-rendition-files-live.md) names. The CLIP job and the face and OCR previews stay as [Decide whether an edited still gets a CLIP vector](31-decide-whether-an-edited-still-gets-a-clip-vector.md) and [Decide which preview face detection and OCR read after an edit](32-decide-which-preview-face-detection-and-ocr-read-after-an-edit.md) decided.

`PUT /api/assets/{id}/edits` returns 200 `{ assetId, edits }`. Each item is `id`, `action`, and `parameters`, in `sequence` order. The app does not read that body. The body requires `edits` with at least one item. Crop `x` and `y` are integers of at least 0. Crop `width` and `height` are integers of at least 1. Rotate `angle` is 0, 90, 180, or 270. Mirror `axis` is `horizontal` or `vertical`. A second crop, a second rotate, or a second mirror with the same `axis` is 400 and stores nothing. Two mirrors with different axes are kept. A crop that is present and is not the first item is 400 `Crop action must be the first edit action`. A crop that extends past the image is 400 `Crop parameters are out of bounds`. The bounds are the exif width and height, swapped when the orientation is `5`, `6`, `7`, `8`, `90`, or `-90`. A missing width or height is 400 `Asset dimensions are not available for editing`.

That write also returns 400 and stores nothing when the type is not an image (`Only images can be edited`), `livePhotoVideoId` is set (`Editing live photos is not supported`), the Asset is a panorama (`Editing panorama images is not supported`), or the original path ends in `.gif` or `.svg` (`Editing GIF images is not supported`, `Editing SVG images is not supported`). A panorama is `projectionType` `EQUIRECTANGULAR`, or a file name ending in `.insp`. `PUT` loads the Asset with its exif row. No exif row is 400 `Asset not found`.

`DELETE /api/assets/{id}/edits` returns an empty 204. It removes every edit for that Asset and queues the same thumbnail job. An Asset with no edits still returns 204 and still queues the job. A miss on the follow-up load is 400 `Asset not found`.

`GET /api/assets/{id}/edits` returns 200 `{ assetId, edits }` with those same edit objects, ordered by `sequence`. The mobile app does not call it. The body matches v3.3.6 so a later screen that does call it sees the same response. Which screens call it stays in the map's Not yet specified section.

`PUT` and `DELETE` succeed for the owner. Anyone else receives 400 `Not found or no asset.edit.create access` or `Not found or no asset.edit.delete access`, and nothing is stored. A missing Asset fails that same check. `GET` succeeds for the owner and for a User who can already read the Asset through an album. A failed read is 400 `Not found or no asset.read access`. A locked Asset fails the owner check. This server has no session elevation that lifts it. A shared-link session receives 403 `Forbidden` on all three routes.

`AssetEditsV1` sends `AssetEditDeleteV1` first, then `AssetEditV1`, for Assets that User owns. The upsert ack cursor is `updateId`. After `AssetEditThumbnailGeneration` finishes, the owner receives `AssetEditReadyV2`. `asset` is the `SyncAssetV2` row, `isEdited` included. `edit` is the `AssetEditV1` list in `sequence` order. A missing Asset sends no event.

A missing required sync field still fails that batch and is not acked, as [Inventory which edit fields the app stores](30-inventory-which-edit-fields-the-app-stores.md) recorded.
