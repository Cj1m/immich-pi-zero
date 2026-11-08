# Decide which person and face columns this server stores

Type: grilling
Status: resolved

## Question

[Decide which person hide and favorite responses this server matches](44-decide-which-person-hide-and-favorite-responses-this-server-matches.md), [Decide which person merge response this server matches](45-decide-which-person-merge-response-this-server-matches.md), and [Decide which face reassignment responses this server matches](46-decide-which-face-reassignment-responses-this-server-matches.md) write the person row and the face row. [Decide whether face boxes are in the Contract](16-decide-whether-face-boxes-are-in-the-contract.md) already sends the face on `AssetFaceV3`. [Inventory where the person thumbnail lives](18-inventory-where-the-person-thumbnail-lives.md) names `thumbnailPath`. v3.3.6 stores these rows in Postgres. This server stores them in SQLite. Which person and face columns does this server keep?

## Comments

Recommendation accepted: keep the v3.3.6 person and face columns. The public person id is `personGroupId`. `clusterGroupId` on that group may be null.

## Answer

`person_group` keeps `id`, `clusterGroupId`, `createdAt`, `createId`, `updatedAt`, and `updateId`. `id` is the person id the API and the sync already use. `clusterGroupId` may be null. This server still does not add a cluster.

`person` keeps `ownerId`, `personGroupId`, `createdAt`, `updatedAt`, `name`, `thumbnailPath`, `isHidden`, `birthDate`, `faceAssetId`, `isFavorite`, `color`, and `updateId`. `ownerId` and `personGroupId` are the primary key. `name` and `thumbnailPath` default to empty. `isHidden` and `isFavorite` default to false. `birthDate`, `faceAssetId`, and `color` are nullable. `birthDate` is a date, and it is not after today. `thumbnailPath` is the file [Inventory where the person thumbnail lives](18-inventory-where-the-person-thumbnail-lives.md) already opens.

`asset_face` keeps `id`, `assetId`, `personGroupId`, `imageWidth`, `imageHeight`, `boundingBoxX1`, `boundingBoxY1`, `boundingBoxX2`, `boundingBoxY2`, `sourceType`, `deletedAt`, `updatedAt`, `updateId`, `isVisible`, `ownerId`, `assetVisibility`, and `assetDeletedAt`. The box, the image size, and `sourceType` are what `AssetFaceV3` already sends. `sourceType` defaults to machine learning. `isVisible` defaults to true. `personGroupId` and `deletedAt` are nullable. `ownerId`, `assetVisibility`, and `assetDeletedAt` copy the Asset so a face query does not join it.

A deleted person writes `person_audit` with `id`, `personGroupId`, `ownerId`, and `deletedAt`. A deleted face writes `asset_face_audit` with `id`, `assetFaceId`, `assetId`, and `deletedAt`. Those ids are the delete sync already decided. Rows older than the sync prune window can go. The ack cursor is the audit `id`.
