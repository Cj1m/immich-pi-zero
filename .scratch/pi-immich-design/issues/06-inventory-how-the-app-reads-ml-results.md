# Inventory how the app reads ML results

Type: research
Status: resolved

## Question

Which calls does the official mobile app in `reference/immich` (tag v3.3.6) make to show faces, people, and search results the server stored after calling the ML service? Include OCR only if the app shows it on this path. The server's role as client of the ML service is already settled in the map Notes; this ticket is the read side of the Contract. Write the inventory to `docs/research/ml-results.md` and link it from this ticket when resolving.

## Answer

People, a person's page, and the faces on an asset are local rows. Search hits stay in memory. Per-call fields are in [Faces, people, and search](../../../docs/research/ml-results.md).

The library card and the people page watch SQLite filled by `POST /api/sync/stream` (`PeopleV1`, `AssetFacesV3`, and `AssetOcrV1` against v3.3.6). They do not call `GET /api/people`. A person page does not call `GET /api/people/{id}`. Its grid is a local face join. The asset details sheet lists each non-hidden person on that asset. The bounding box is stored and not drawn. There is no merge, hide, favorite, or face-reassign call under `mobile/`.

Saving a name or a birthday is `PUT /api/people/{id}`. The local write runs only after the call returns. The app does not send `isHidden`, `isFavorite`, or a featured face. Every avatar is `GET /api/people/{id}/thumbnail`, with `c` set to `updatedAt` when the row has one.

Context text, and "View similar photos" (`queryAssetId`), are `POST /api/search/smart`. Filename, description, OCR text, and a filter-only search are `POST /api/search/metadata`. Camera and location dropdowns are `GET /api/search/suggestions`. The context item is hidden unless server features say `smartSearch`. OCR text mode is hidden unless they say `ocr`. Hits are not written back to SQLite. The tile then uses the same asset thumbnail route as the library.

OCR is shown. The viewer draws boxes from local OCR rows and the selected box's text. There is no `GET /api/assets/{id}/ocr`.

The birthday body is a UTC calendar day. The schema format is `date`. The generated serializer is not in the checkout, so the exact JSON spelling of that day is not pinned.
