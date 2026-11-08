# Decide when the web editor finishes a save

Type: grilling
Status: resolved

## Question

[Decide which edit fields this server stores](37-decide-which-edit-fields-this-server-stores.md) already matches `GET`, `PUT`, and `DELETE /api/assets/{id}/edits`, and the owner receives `AssetEditReadyV2` after the thumbnail job. The web editor waits on that websocket event before it treats the save as done. The face editor draws boxes from the face list and calls `POST /api/faces`. The call shapes are in [Remaining to close](../../../docs/research/remaining-to-close.md). Where the web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Is a web save unfinished until `AssetEditReadyV2` arrives, and does `POST /api/faces` match v3.3.6?

## Comments

Recommendation accepted: the web save stays unfinished until `AssetEditReadyV2` for that Asset arrives, or until 10 seconds pass. `POST /api/faces` matches v3.3.6.

## Answer

A web save is unfinished until `AssetEditReadyV2` arrives for that Asset. The editor registers the wait, then calls the PUT or DELETE already decided in [Decide which edit fields this server stores](37-decide-which-edit-fields-this-server-stores.md). An empty edit list is the DELETE. Any other list is the PUT. The wait is 10 seconds. The event's `asset.id` must be that Asset. When it arrives, the editor emits `AssetEditsApplied` and shows the success toast. When 10 seconds pass with no event, the editor shows the error toast. The rows from the PUT or DELETE are already stored either way.

The event is sent to the owner after `AssetEditThumbnailGeneration` finishes, the same moment [Decide which edit fields this server stores](37-decide-which-edit-fields-this-server-stores.md) already names. The payload is the asset snapshot the mobile timeline already reads (`id`, `ownerId`, `originalFileName`, `thumbhash`, `checksum`, `fileCreatedAt`, `fileModifiedAt`, `createdAt`, `localDateTime`, `duration`, `type`, `deletedAt`, `isFavorite`, `visibility`, `livePhotoVideoId`, `stackId`, `libraryId`, `width`, `height`, `isEdited`) and `edit`, the `AssetEditV1` list. The web page reads only `asset.id`.

`POST /api/faces` matches v3.3.6. The face editor draws the box in the browser from the face list. It does not need a new image to draw it. Tagging a person sends `assetId`, `personId`, `imageWidth`, `imageHeight`, `x`, `y`, `width`, and `height`, all integers, in the preview the editor is showing. The call returns 201 with an empty body. The page reads no field. A missing Asset is 404 `Asset not found`. A missing person is 400 `Person not found`. An edited still whose dimensions are missing is 400 `Asset does not have valid dimensions`.

The stored box is in the unedited photo's coordinates. On an edited still the server scales the preview box by `asset.width / imageWidth`, inverts the crop, rotate, and mirror, and rounds the corners. `sourceType` is `manual`. The row syncs as `AssetFaceV3`. The phone stores the box and still does not draw it. When that person has no thumbnail asset, the server queues `PersonGenerateThumbnail`. Reassignment and delete of a face stay on [Decide which face reassignment responses this server matches](46-decide-which-face-reassignment-responses-this-server-matches.md).
