# Inventory which signed-in photo routes add a response

Type: grilling
Status: resolved

## Question

The photo routes under the signed-in shell are library, albums, trash, search, people, map, and the screens beside them. Where the official mobile app or the official web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do any of those routes add an HTTP response the decided calls do not already cover?

## Comments

Recommendation accepted: the shell reuses the calls already decided, and it also adds the families named below. Each family matches v3.3.6 unless a later ticket finds the Pi cannot do it. License and purchase stay with the admin jobs fog.

## Answer

Yes. The library, albums, trash, search, people, map, and the screens beside them add HTTP responses the decided calls do not cover.

These screens reuse calls that are already decided. Photos, archive, favorites, locked, recently added, a person, a partner, an album, the map panel, and search still load a timeline, and the shared-link open already matched `GET /api/timeline/buckets` and `GET /api/timeline/bucket` for a share session. The signed-in query is a different caller, so it is still a new response. Search also sends `POST /api/search/metadata` and `POST /api/search/smart`. The viewer still loads thumbnail, preview, Original, and playback. Favorites, stacks, duplicates, memories, partners, shared links, activities, edits, and the people writes (update, merge, create, reassign, delete) stay on their tickets. Album create, update, asset add and remove, user add and remove, and album delete are the same paths [Inventory albums, trash, and shared links](05-inventory-albums-trash-and-shared-links.md) recorded for the phone. The web also reads the list and the album document, which that inventory did not lock.

The new families are:

- Signed-in `GET /api/timeline/buckets` and `GET /api/timeline/bucket`, plus `GET /api/assets/{id}`, `GET /api/assets/{id}/ocr`, and `GET /api/faces`.
- `GET /api/map/markers`, `GET /api/search/cities`, `GET /api/search/explore`, `GET /api/search/places`, `POST /api/search/large-assets`, and `GET /api/view/folder` with `GET /api/view/folder/unique-paths`.
- `GET /api/albums`, `GET /api/albums/{id}`, and `GET /api/albums/statistics`. The web album page reads those documents. Album map markers are the same route the shared-link open already matched.
- `POST /api/trash/empty`, `POST /api/trash/restore`, and `POST /api/trash/restore/assets`, and `PUT /api/assets` for a date, a description, a rating, a GPS point, or visibility. Favorite and `duplicateId` stay on their tickets.
- `GET /api/people`, `GET /api/people/{id}`, `GET /api/people/{id}/statistics`, `GET /api/search/person`, and `GET`, `PUT`, and `DELETE /api/people/users`.
- `GET`, `PUT`, and `DELETE /api/tags`, `PUT /api/tags/assets`, and `PUT` and `DELETE /api/tags/{id}/assets`. A tag change is one of the writes that queues a sidecar.
- `GET /api/auth/status`, `POST`, `PUT`, and `DELETE /api/auth/pin-code`, and `POST /api/auth/session/lock` and `POST /api/auth/session/unlock`.
- `GET`, `POST`, `PUT`, and `DELETE /api/api-keys`, `GET /api/sessions`, `DELETE /api/sessions` and `DELETE /api/sessions/{id}`, `POST /api/users/profile-image`, `GET` and `PUT /api/notifications`, `PUT /api/users/me/preferences` from the settings page, `GET /api/assets/statistics`, and `GET /api/users/me/calendar-heatmap`.
- `GET`, `POST`, `PUT`, and `DELETE /api/workflows`, the workflow logs and triggers, and `GET /api/plugins/methods` and `GET /api/plugins/templates`.
- The cluster-group routes Sharing settings calls. [Decide which later-user responses this server matches](36-decide-which-later-user-responses-this-server-matches.md) still holds: creating a User does not add a cluster, and `clusterGroupId` may be null.
- `POST /api/assets/bulk-upload-check`, `GET /api/server/media-types`, `POST /api/assets/jobs`, and the signed-in `POST /api/download/info` and `POST /api/download/archive`.

`GET` and `PUT /api/users/me/license`, `DELETE /api/users/me/license`, and the buy page stay in the purchase fog with the admin jobs screen.
