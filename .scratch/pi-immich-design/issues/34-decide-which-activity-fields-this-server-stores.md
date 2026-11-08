# Decide which activity fields this server stores

Type: grilling
Status: resolved

## Question

[Inventory album activity](25-inventory-album-activity.md) names the comments-screen calls and what a missing field does. List and create return `id`, `createdAt`, nullable `assetId`, `type` (`comment` or `like`), and `user` (`avatarColor`, `email`, `id`, `name`, `profileChangedAt`, `profileImagePath`). `comment` may be absent. The response has no `albumId`. The app stamps the album it asked for. Create sends `albumId` and `type`, plus `assetId` and `comment` only when it has them. A second like by the same user for that album or asset returns the existing row with status 200. A new row is 201. Delete is `DELETE /api/activities/{id}` with an empty 204 body, which the app treats as success. A missing required list field is an empty screen, and the text field stays. The phone keeps these rows in memory only. They are not on the sync stream. The comments-and-likes switch is still `isActivityEnabled` on the album. Which of these fields does this server keep, and which does each response include?

## Comments

The row matches v3.3.6. This server keeps `id`, `createdAt`, `updatedAt`, `albumId`, `userId`, `assetId`, `comment`, `isLiked`, and `updateId`. `assetId` null is the album itself. A like is `isLiked` true and `comment` null. A comment is `isLiked` false and `comment` set.

Recommendation accepted: list and create return `id`, `createdAt`, `assetId`, `comment`, `type`, and `user`. `type` is `like` or `comment` from `isLiked`. `user` is that User's `id`, `name`, `email`, `avatarColor`, `profileImagePath`, and `profileChangedAt`. The body is those fields. A new row is 201. A second like by the same User for that album and the same asset, or for the album itself, returns the existing row with 200 and does not insert. Delete is an empty 204.

Recommendation accepted: `GET /api/activities` returns the v3.3.6 set. With `albumId` and no `assetId`, that is every row on the album, album-level and asset-level, oldest `createdAt` first. With `assetId`, that is only that asset's rows. A trashed asset's rows stay stored and drop off the list until the asset is restored. An album-level row stays. A deleted User's rows drop off the list. `level`, `type`, and `userId` apply when a client sends them.

Recommendation accepted: any album member can create while `isActivityEnabled` is true. Otherwise create returns 400 `Not found or no activity.create access` and stores nothing. The signed-in User is `userId`. A comment needs non-blank text. A like includes no comment. A body that breaks either rule is 400 and stores nothing. The list does not read the switch. A member who can read the album still gets the rows. A failed read is 400 `Not found or no album.read access`. Delete succeeds for the User who left the row and for the album owner. Anyone else gets 400 `Not found or no activity.delete access`, and the row stays.

Recommendation accepted: deleting the album deletes its activities. Deleting the User deletes the activities they left. Hard-deleting an asset deletes activities that name it. Trashing the asset leaves the rows stored. Removing the asset from the album deletes activities for that pair. An album-level row stays through that removal.

Recommendation accepted: `GET /api/activities/statistics` returns 200 `{ comments, likes }` for an album the caller can read. `comments` counts `isLiked` false. `likes` counts `isLiked` true. A deleted User, a trashed asset, and a locked asset are left out of the counts. Album-level rows count. With `assetId`, only that asset counts. The switch is not read. A shared-link session gets 403 `Forbidden` on every activity route.

## Answer

This server keeps one activity row per like or comment. The columns match v3.3.6: `id`, `createdAt`, `updatedAt`, `albumId`, `userId`, `assetId`, `comment`, `isLiked`, and `updateId`. `assetId` null is the album itself. A like is `isLiked` true and `comment` null. A comment is `isLiked` false and `comment` set. Activity stays off the sync stream. `updateId` remains on the row. `isActivityEnabled` stays on the album.

`GET /api/activities` and `POST /api/activities` return `id`, `createdAt`, `assetId`, `comment`, `type`, and `user`. `type` is `like` when `isLiked` is true, and `comment` otherwise. `user` is the joined User: `id`, `name`, `email`, `avatarColor`, `profileImagePath`, and `profileChangedAt`. The app stamps the album it asked for.

A new row is 201. A second like by the same User for that album and the same asset, or for the album itself, returns the existing row with 200 and does not insert. `DELETE /api/activities/{id}` is an empty 204.

The list requires `albumId`. With no `assetId`, it returns every row on the album, album-level and asset-level, oldest `createdAt` first. With `assetId`, it returns only that asset's rows. A trashed asset's rows stay stored and drop off the list until the asset is restored. An album-level row stays. A deleted User's rows drop off the list. `level`, `type`, and `userId` apply when a client sends them.

Any album member can create while `isActivityEnabled` is true. Otherwise create returns 400 `Not found or no activity.create access` and stores nothing. The signed-in User is `userId`. A comment needs non-blank text. A like includes no comment. A body that breaks either rule is 400 and stores nothing. The list leaves the switch unread. A member who can read the album still gets the rows. A failed read is 400 `Not found or no album.read access`. Delete succeeds for the User who left the row and for the album owner. Anyone else gets 400 `Not found or no activity.delete access`, and the row stays.

Deleting the album deletes its activities. Deleting the User deletes the activities they left. Hard-deleting an asset deletes activities that name it. Trashing the asset leaves the rows stored. Removing the asset from the album deletes activities for that pair. An album-level row stays through that removal.

`GET /api/activities/statistics` returns 200 `{ comments, likes }` for an album the caller can read. `comments` counts `isLiked` false. `likes` counts `isLiked` true. A deleted User, a trashed asset, and a locked asset are left out of the counts. Album-level rows count. With `assetId`, only that asset counts. The switch is unread here too. The phone does not call this route. The web activity manager does.

A shared-link session gets 403 `Forbidden` on every activity route.

A missing required list field still empties the comments screen, and the text field stays, as [Inventory album activity](25-inventory-album-activity.md) recorded.
