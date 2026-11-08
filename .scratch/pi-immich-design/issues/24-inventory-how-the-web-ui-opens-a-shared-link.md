# Inventory how the web UI opens a shared link

Type: research
Status: resolved

## Question

Which HTTP calls does the v3.3.6 web UI in `reference/immich` make when a recipient opens `/s/{slug}` or `/share/{key}`? Include a password-protected link and a link that does not resolve. Name the method, path, and the fields the page requires. [Decide whether a recipient can open a shared link](14-decide-whether-a-recipient-can-open-a-shared-link.md) puts this open in the destination. [Inventory albums, trash, and shared links](05-inventory-albums-trash-and-shared-links.md) records that the mobile app never makes these calls. Write the note to `docs/research/shared-link-open.md` and link it from this ticket when resolving.

## Answer

A recipient's open is the document, then the public boot calls, then the link. Per-call fields are in [Shared link open](../../../docs/research/shared-link-open.md).

`GET /s/{slug}` or `GET /share/{key}` returns `index.html`. A link with no password is 200 and may include Open Graph tags. A password-protected link is 200 with those tags left empty. A missing, expired, or deleted-album link is 404, and the body is still `index.html`, so the client still boots. A query string or an extra `/photos/...` segment is part of the document lookup and makes that document 404.

Boot is `GET /api/server/config`, then `GET /api/server/features`. A signed-in browser also calls `GET /api/users/me` and `GET /api/users/me/preferences`. A recipient with no `immich_is_authenticated` cookie does not.

The page loads `GET /api/shared-links/me` with `key` or `slug`. `401` and `{ "message": "Password required" }` is the password form and stops. Any other failure is the not-found page, including `Invalid share key` and `Invalid share slug`. Submit is `POST /api/shared-links/login` with `{ "password" }`. Success caches the body, so the next navigation does not call `me` again.

An album grid is `GET /api/timeline/buckets`, then `GET /api/timeline/bucket` for each visible month, then `GET /api/assets/{id}/thumbnail?size=thumbnail&edited=true`. When `showMetadata` and `features.map` are both true, `GET /api/albums/{id}/map-markers` fires on mount. An individual link with upload allowed, or with more than one Asset, draws thumbnails from the `assets` already on the link. Exactly one Asset and `allowUpload` false calls `GET /api/assets/{id}` and opens the viewer: a still loads thumbnail then preview, a video sets `src` to `video/playback` unless realtime transcoding is on.

Download, upload, slideshow, the map modal, comments, hover playback, and opening one Asset from the grid are not part of this open.
