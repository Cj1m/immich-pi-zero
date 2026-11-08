# Inventory thumbnails and originals

Type: research
Status: resolved

## Question

Which thumbnail requests and Original byte-range requests does the official mobile app in `reference/immich` (tag v3.3.6) make when opening a photo and playing a video? Record each rendition the app asks for and how a video seek is expressed. Originals are streamed and never transcoded. Write the inventory to `docs/research/thumbnails-and-originals.md` and link it from this ticket when resolving.

## Answer

A remote photo is two thumbnail calls, then maybe the Original. A remote video plays `video/playback` unless a setting says otherwise. Per-call fields are in [Thumbnails and originals](../../../docs/research/thumbnails-and-originals.md).

A device file skips HTTP. `preferRemote` defaults off, and an edited Asset does not use the local still. A video with a readable local file, or a live photo's motion file, is played from disk.

The tile and the viewer's placeholder are `GET /api/assets/{id}/thumbnail?size=thumbnail&edited=true&c=<thumbhash>`. Opening a remote photo then requests `size=preview` and stops. `loadOriginal` defaults off. The Original (`GET /api/assets/{id}/original?edited=true`) is requested only when that switch is on, and always for an animated still (`durationMs > 0`), because the preview does not animate. Pinch zoom does not ask for another rendition. The viewer never sends `size=fullsize`. `edited=true` is the viewer's default. It is not tied to the row's `isEdited`.

A remote video's poster is that same preview. The player URL is `GET /api/assets/{id}/video/playback` with no query. `loadOriginalVideo` defaults off. When it is on, the URL is `GET /api/assets/{id}/original` with no query, so the server treats `edited` as false. A live photo's player id is `livePhotoVideoId`. There is no fallback from playback to the Original. Playback serves the encoded video file when one exists, and the Original path otherwise. The encoded file is not the Original.

Seek does not change the URL. The slider seeks the player to a position in milliseconds. Android Media3 sends `Range: bytes=<position>-` or `bytes=<position>-<end>`. iOS seeks `AVPlayer` and does not set `Range` in this app. Both file routes go through `sendFile`, which answers one valid `bytes=` range with 206. The app does not read 200 versus 206. Image requests do not send `Range`. They download the whole rendition.
