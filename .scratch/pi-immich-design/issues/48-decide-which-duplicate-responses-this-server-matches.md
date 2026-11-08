# Decide which duplicate responses this server matches

Type: grilling
Status: resolved

## Question

The web duplicates screen uses `GET /api/duplicates`, `DELETE /api/duplicates/{id}`, and `POST /api/duplicates/resolve`. Grouping uses `machineLearning.duplicateDetection` and the stored CLIP vector. It does not run a separate model on the Pi. The call shapes are in [Remaining to close](../../../docs/research/remaining-to-close.md). Where the web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do those three calls match v3.3.6?

## Comments

Recommendation accepted: the three calls match v3.3.6. Grouping uses the stored CLIP vector and `machineLearning.duplicateDetection`. There is no separate duplicate table and no separate model.

## Answer

The three calls match v3.3.6. The phone does not send them. The one Pi difference is the search: exact cosine, as [Decide how this server stores the CLIP vector](26-decide-how-this-server-stores-the-clip-vector.md) already decided, instead of an approximate index. The threshold and the limit stay v3.3.6.

`GET /api/server/features` reports `duplicateDetection` when `machineLearning.enabled`, `clip.enabled`, and `duplicateDetection.enabled` are all true. The default is enabled, with `maxDistance` 0.01. Those keys are already on the admin config document.

After an upload writes a CLIP vector, the server queues duplicate detection for that Asset. The job skips a stacked Asset, a hidden Asset, and a locked Asset. A missing vector fails that Asset and writes no group. Otherwise it compares the owner's other Assets of the same type that are not deleted and not stacked, visibility timeline or archive, nearest 64 by cosine distance. A distance at or under `maxDistance` joins one `duplicateId`: an id the set already shares, or a new id. No hit clears `duplicateId` when one was set. The job stores `duplicatesDetectedAt` so a later scan skips that Asset unless a forced run asks again.

A group is that shared `duplicateId`. There is no duplicate table.

`GET /api/duplicates` returns 200. A group of one loses `duplicateId` before the list is built. The list is groups of two or more, owned by the caller, not deleted, not stacked, visibility timeline or archive. Each item is `{ duplicateId, assets, suggestedKeepAssetIds }`. Each asset is the asset document. The compare view reads `id`, `originalFileName`, `originalPath`, file size, resolution, `fileCreatedAt`, `fileModifiedAt`, and the exif fields that differ: `dateTimeOriginal`, `timeZone`, `city`, `state`, `country`, `latitude`, `longitude`, `make`, `model`, `lensModel`, `fNumber`, `focalLength`, `iso`, `exposureTime`, `description`, `rating`, `orientation`, `projectionType`. `suggestedKeepAssetIds` is one id: the largest `fileSizeInByte`, and among those the most filled-in exif values. The page reads `duplicateId`, `assets`, and `suggestedKeepAssetIds`.

`DELETE /api/duplicates/{id}` and `DELETE /api/duplicates` with `{ ids }` both return 204 and an empty body. Both clear `duplicateId` on the caller's Assets in those groups. The keep-all button sends the bulk form. A group the caller does not own is 400 `Not found or no duplicate.delete access`.

`POST /api/duplicates/resolve` with `{ groups: [{ duplicateId, keepAssetIds, trashAssetIds }] }` returns 200 and one `{ id, success, error?, errorMessage? }` per group, in that order. `id` is the `duplicateId`. The page reads `success`, and on one group also `error` and `errorMessage`. An empty `groups` list is 400. A group the caller does not own fails the whole call with 400 `Not found or no duplicate.delete access`.

For one group, an id that is not in the group is ignored. An Asset in both lists is `{ success: false, error: "validation", errorMessage: "An asset cannot be in both keepAssetIds and trashAssetIds" }`. An Asset in neither list is the same error with `Every asset must be in either keepAssetIds or trashAssetIds`. A missing group is `{ success: false, error: "not_found" }`. Trash ids the caller cannot delete are `{ success: false, error: "no_permission", errorMessage: "No permission to delete assets" }`. Any other failure is `{ success: false, error: "unknown" }`.

When exactly one Asset is kept and at least one is trashed, that keeper receives the merge: `isFavorite` true if any Asset in the group is a favorite; visibility locked, else archive, else timeline, else hidden, from the Assets that have one; the highest rating; descriptions joined as unique non-empty lines; latitude and longitude only when every Asset that has a coordinate agrees. The keeper is added to each album any Asset in the group is already in, when the caller can add it. Tags merge the same way when the Assets have them. `duplicateId` is cleared on the keeper. An exif or tag change queues a sidecar write. When more than one Asset is kept, the only write is clearing `duplicateId`.

Trashed ids follow `trash.enabled`. When trash is on, they are trashed. When trash is off, they are deleted. Either way `duplicateId` is cleared. `GET /api/server/features` reports `trash` from that same flag, which is the confirm copy this page shows.

The stack button sends `POST /api/stacks` with `{ assetIds }` (at least two, the first is the primary) and then `PUT /api/assets` with `{ ids, duplicateId: null }`. The create is 201. The bulk update is 204 and empty. The page reads neither body and drops the group. An Asset the caller cannot update is 400 `Not found or no asset.update access`.

An unsigned call is 401. A shared-link session is 403 `Forbidden`.
