# Inventory where smart-search embeddings live

Type: research
Status: resolved

## Question

Where does the v3.3.6 server in `reference/immich` store the vectors that `POST /api/search/smart` reads, both for a text `query` and for `queryAssetId`? Name the tables. Say whether a missing vector changes the response the app sees. The server's calls to the ML service are already settled in the map Notes; this ticket is where the stored vectors live. [Inventory how the app reads ML results](06-inventory-how-the-app-reads-ml-results.md) records the request. Write the note to `docs/research/search-embeddings.md` and link it from this ticket when resolving.

## Answer

`POST /api/search/smart` ranks rows in `smart_search`. Each row is one `assetId` and one `embedding`. The text `query` is not stored. The handler asks the ML service for a `clip` string when the search runs, and compares that string. `queryAssetId` loads the asset's row. A non-empty `query` does not read `queryAssetId`. `face_search` and `ocr_search` are not this store.

A missing `smart_search` row for `queryAssetId` is 400 `{ message: 'Asset <id> has no embedding' }`. A text call that throws is 500 `{ message: 'Failed to search smart' }`. Smart search disabled is 400 `{ message: 'Smart search is not enabled' }`. An empty table, with a query vector in hand, is 200 and `assets.items` empty with `nextPage` null. Assets with no row are left out. They are not sorted last. The app's search returns null for that empty page and for those errors, so a new search draws no hits and does not show the message.

The job `SmartSearch` writes the row from the preview file. Deleting the asset removes it. A CLIP model change deletes every row.

The table, the query vector, and those responses are in [Search embeddings](../../../docs/research/search-embeddings.md).
