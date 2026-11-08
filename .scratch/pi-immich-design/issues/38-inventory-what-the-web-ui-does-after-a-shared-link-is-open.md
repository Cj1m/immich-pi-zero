# Inventory what the web UI does after a shared link is open

Type: research
Status: resolved

## Question

[Decide which shared-link open responses this server matches](33-decide-which-shared-link-open-responses-this-server-matches.md) stops at first paint. [Shared link open](../../../docs/research/shared-link-open.md) names the later actions and does not specify their fields: download, upload, slideshow, the map modal, comments, hover playback, and opening one Asset from the grid. Which HTTP calls does the v3.3.6 web UI make for each of those actions, and which fields does the page require? Where a call is one [Inventory album activity](25-inventory-album-activity.md), the playback route already named, or a route [Decide which activity fields this server stores](34-decide-which-activity-fields-this-server-stores.md) already matches, say so. That decision includes `GET /api/activities/statistics`. A shared-link session gets 403 `Forbidden` on every activity route. Write the note to `docs/research/shared-link-after-open.md` and link it from this ticket when resolving.

## Answer

Per-call fields are in [Shared link after open](../../../docs/research/shared-link-after-open.md).

Download of an album, of every Asset on an individual link, or of a multi-select of more than one Asset is `POST /api/download/info`, then `POST /api/download/archive`. One selected Asset, and the viewer download, is `GET /api/assets/{id}/original`. A shared-link session forces `edited` true on that Original route.

Upload is `POST /api/assets` with the share `key` or `slug`. The page sends `fileCreatedAt`, `fileModifiedAt`, `isFavorite`, and `assetData`. It does not send a checksum, and it does not call `POST /api/assets/bulk-upload-check`. The extension list comes from `GET /api/server/media-types`, which app init already fired. The page does not refetch the link. The server attaches the Asset to the link.

Slideshow, the map modal, and opening one Asset reuse `GET /api/assets/{id}`, the timeline bucket, and the thumbnail and preview URLs from the open. The map modal also loads the style URL already on the server config. Hover is `GET /api/assets/{id}/video/playback`. That playback route is the one already named: the Original, status 200 or 206. `realtimeTranscoding` is false, so the viewer does not request HLS. Hover does not read that flag.

The shared-link pages do not call the activity routes. Those routes stay as [Decide which activity fields this server stores](34-decide-which-activity-fields-this-server-stores.md) left them, including `GET /api/activities/statistics` and 403 `Forbidden` for a shared-link session.

An owner signed in on an individual link can `DELETE /api/shared-links/{id}/assets`. Cast makes no Immich call.
