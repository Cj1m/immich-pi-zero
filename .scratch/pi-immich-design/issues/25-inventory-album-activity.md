# Inventory album activity

Type: research
Status: resolved

## Question

[Decide whether album activity is in the Contract](15-decide-whether-album-activity-is-in-the-contract.md) puts album activity in the Contract. The owner sends `isActivityEnabled` on `PATCH /api/albums/{id}`. On a shared album with that switch on, the comments screen lists, creates, and deletes a like or a comment, on the album or on one Asset in it. Which HTTP calls does the v3.3.6 mobile app make for that screen? Name the method, path, and the fields the app requires. Say what a missing field does to the switch and to the comments screen. [Inventory albums, trash, and shared links](05-inventory-albums-trash-and-shared-links.md) recorded the switch and left these calls out. Write the note to `docs/research/album-activity.md` and link it from this ticket when resolving.

## Answer

The comments screen lists with `GET /api/activities`, creates a like or a comment with `POST /api/activities`, and deletes with `DELETE /api/activities/{id}`. `type` is `like` or `comment`. The album page and the activity deep link send no `assetId`. The viewer sends that asset's id on the list and on a comment. The like button sends the viewer's current asset, not the route's asset id. A missing required list field becomes an empty screen. The text field stays, enabled from the route album's flag. A failed create leaves the list unchanged, and the comment text is already cleared. The empty 204 body throws `NoResponseDtoError`, and the in-memory row is still removed. Any other delete error leaves the row. The phone does not write these rows to SQLite, and activity is not a sync line. `GET /api/activities/statistics` is not called.

The comments-and-likes switch is still `PATCH /api/albums/{id}` with only `isActivityEnabled`. A missing required field on that album response throws before the stored flag is written. The switch stays flipped for that visit. The next open reads the old flag. A missing `isActivityEnabled` on `AlbumsV2` fails that sync batch and does not ack it.

The album page shows the comments button when `isActivityEnabled` is true. Its `isShared` flag is true for every album that read returns, so the button is gated by the activity flag. Only the owner sees the switch.

The calls, the required fields, and those outcomes are in [Album activity](../../../docs/research/album-activity.md). Which of those fields this server keeps is [Decide which activity fields this server stores](34-decide-which-activity-fields-this-server-stores.md).
