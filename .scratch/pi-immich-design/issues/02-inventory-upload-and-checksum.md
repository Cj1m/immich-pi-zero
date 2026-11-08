# Inventory upload and checksum

Type: research
Status: resolved

## Question

Which calls does the official mobile app in `reference/immich` (tag v3.3.6) make to decide an Asset is already on the server and to upload a photo or video? Record the checksum algorithm, the duplicate response, whether the app uses resumable upload, and how the bytes are sent. Say where that agrees or conflicts with the glossary: one Asset per User per set of bytes, Original in object storage, no second copy for a repeat backup by the same User. Write the inventory to `docs/research/upload-and-checksum.md` and link it from this ticket when resolving.

## Answer

The app at v3.3.6 does not ask the server whether a checksum exists before sending bytes. Per-call fields are in [Upload and checksum](../../../docs/research/upload-and-checksum.md).

A backup candidate is skipped when its local base64 SHA-1 equals a remote asset owned by this user. Trash still counts. The hash is of the media bytes. It is not a form field, and the app does not send `x-immich-checksum`. It does not call `POST /api/assets/bulk-upload-check`.

Otherwise it sends one multipart `POST /api/assets` with the file part `assetData`. A live photo is two of those: a hidden motion video, then the still with `livePhotoVideoId`. HTTP 200 and 201 are both success when `id` is a string. The app does not read `status`. There is no tus, chunk, or upload session. A lost foreground connection resends the whole POST once. iOS background sets `retries: 3` on that same POST.

That matches one Asset per User per file checksum, and no second row when the same User repeats a backup. It is narrower for trash (the checksum stays taken), a live photo (two assets), and a phone edit (a new upload, then `POST /api/stacks`). The app never names object storage. The reference server writes the original under its media-location `upload` folder. Embedding duplicate detection is a later job this path does not call.
