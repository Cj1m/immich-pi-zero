# Inventory albums, trash, and shared links

Type: research
Status: resolved

## Question

Which calls does the official mobile app in `reference/immich` (tag v3.3.6) make to create, list, and update albums, add and remove assets, trash, restore, permanently delete, and create and open shared links? For each call, record the method, path, and the fields the app requires. Write the inventory to `docs/research/albums-trash-and-shared-links.md` and link it from this ticket when resolving.

## Answer

Album list, album detail, and trash are local rows. Shared links are their own HTTP list. The app copies a share URL and never opens it. Per-call fields are in [Albums, trash, and shared links](../../../docs/research/albums-trash-and-shared-links.md).

The albums tab and an open album read SQLite written by `POST /api/sync/stream`. They do not call `GET /api/albums` or `GET /api/albums/{id}`. Create is `POST /api/albums` (`albumName`, `assetIds`, and `description` only from the create page). Rename, description, order, cover, and the activity switch are separate `PATCH /api/albums/{id}` bodies. Add is `PUT /api/albums/{id}/assets`. Remove is `DELETE /api/albums/{id}/assets`. Invite is `PUT /api/albums/{id}/users` with user ids and no role. Remove and leave are `DELETE /api/albums/{id}/user/{userId}`. Delete album is `DELETE /api/albums/{id}` and does not trash the assets.

Trash is local rows with `deletedAt` set. There is no trash list route. Trash and permanent delete are both `DELETE /api/assets`. `force` false trashes. `force` true deletes. Restore of a selection is `POST /api/trash/restore/assets`. Restore all is `POST /api/trash/restore`. Empty trash is `POST /api/trash/empty`.

Shared links are not in the sync stream. The library page lists them with `GET /api/shared-links`, creates with `POST /api/shared-links`, edits with `PATCH /api/shared-links/{id}`, and deletes with `DELETE /api/shared-links/{id}`. The app copies `{externalDomain or server}/s/{slug}`, or `{base}/share/{key}` when there is no slug. It never calls the public share routes.
