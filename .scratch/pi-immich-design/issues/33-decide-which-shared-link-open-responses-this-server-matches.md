# Decide which shared-link open responses this server matches

Type: grilling
Status: resolved

## Question

[Decide whether a recipient can open a shared link](14-decide-whether-a-recipient-can-open-a-shared-link.md) puts this open in the destination. [Inventory how the web UI opens a shared link](24-inventory-how-the-web-ui-opens-a-shared-link.md) names every call from the document through the first paint, including a password-protected link and a link that does not resolve. Where the Web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Which of those calls does this server answer the way v3.3.6 does, and which response changes because the Pi cannot do it?

## Comments

Recommendation accepted for the whole open. Every call returns the v3.3.6 status and body. Playback of one shared video is the Original.

## Answer

This server answers every call in [Shared link open](../../../docs/research/shared-link-open.md) the way v3.3.6 does. The Pi changes none of them.

A shared video on first paint uses `GET /api/assets/{id}/video/playback`. The body is the Original. Status is 200, or 206 when the player sends `Range`. `realtimeTranscoding` is false, so the viewer does not request HLS. That is the response v3.3.6 returns when no encoded file exists. This server never writes an encoded file.

`GET /s/{slug}` and `GET /share/{key}` return `index.html`, as [Decide whether this server serves the official web build](23-decide-whether-this-server-serves-the-official-web-build.md) decided. A link with no password is 200, and the server fills the Open Graph tags. A password-protected link is 200 with those tags left empty. A missing, expired, or deleted-album link is 404, and the body is still `index.html`. A query string or an extra path segment is part of the lookup and is 404. `Cache-Control` is `no-store`. `Accept` that does not allow `text/html` is 406.

`GET /api/server/config` is 200 with `maintenanceMode` false. `GET /api/server/features` is 200 with `map` true and `realtimeTranscoding` false. `passwordLogin` and `oauth` stay as [Decide whether OAuth is in the Contract](09-decide-whether-oauth-is-in-the-contract.md) set them. This open does not read the other config or feature fields.

A recipient with no `immich_is_authenticated` cookie does not call `GET /api/users/me`, `GET /api/users/me/preferences`, or `GET /api/server/about`. When the cookie is present, those calls match v3.3.6. The share page reads `user.id`. A failure of the pair is dropped and the share continues. About runs only when `user.license.activatedAt` is missing, and a failure is ignored. The later-user screens stay on [Decide which later-user responses this server matches](36-decide-which-later-user-responses-this-server-matches.md).

`GET /api/shared-links/me` with `key` or `slug` is 200 and the link when the lookup succeeds and the password check passes. `401` and `{ "message": "Password required" }` is the password form and stops. `401` and `{ "message": "Invalid share key" }` or `{ "message": "Invalid share slug" }` is the not-found page. `400` and `Shared link not found` is the case where auth passed and the row is gone. `showMetadata` false strips each asset the way v3.3.6 does. The stored password string is on the JSON. The form is the 401, not that field.

`POST /api/shared-links/login` with `{ "password" }` is 201 and the same link body. The handler sets `immich_shared_link_token` to the existing comma-separated tokens plus the base64 SHA-256 of `{id}-{password}` when that token is not already there. The cookie is `HttpOnly`, `Path=/`, `SameSite=Lax`, `Max-Age` one day, and `Secure` when the request is HTTPS. A wrong password is 401 `{ "message": "Invalid password" }`. The form stays up and the page toasts that message. A link with no password is 400 `{ "message": "Shared link is not password protected" }`. No link on the auth object is 403 with an empty message. The token is checked from the cookie. This server stores no share session.

An album grid is `GET /api/timeline/buckets`, then `GET /api/timeline/bucket` for each visible month, then `GET /api/assets/{id}/thumbnail?size=thumbnail&edited=true` with the share `key` or `slug`. The timeline JSON matches v3.3.6. `showMetadata` false forces coordinates off. When `showMetadata` and `map` are both true, `GET /api/albums/{id}/map-markers` is 200 and the marker array. The page does not read that array until the map button is clicked.

An individual link uses the `assets` already on the link. Exactly one Asset and `allowUpload` false calls `GET /api/assets/{id}`. That body matches v3.3.6, including the stripped asset when `showMetadata` is false, no `owner`, and `people` set to `[]`. A still then loads the thumbnail and the preview. A video uses the playback response above. `size=original` is not this first paint.

Download, upload, slideshow, the map modal, comments, hover playback, and opening one Asset from the grid are [Inventory what the web UI does after a shared link is open](38-inventory-what-the-web-ui-does-after-a-shared-link-is-open.md).
