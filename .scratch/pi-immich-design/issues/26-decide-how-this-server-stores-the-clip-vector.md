# Decide how this server stores the CLIP vector

Type: grilling
Status: resolved

## Question

`POST /api/search/smart` ranks assets by cosine distance against `smart_search.embedding`. The text query is not stored. The server asks the ML service for that vector when the search runs. [Inventory where smart-search embeddings live](19-inventory-where-smart-search-embeddings-live.md) names the table, the job that writes it, and the responses a missing vector produces. This server uses SQLite and does not run pgvector. Where does the vector live, and how does the ranking return the assets the app sees?

## Comments

Recommendation accepted. One CLIP vector per Asset is a SQLite blob of float32 values. The server ranks those blobs by exact cosine distance and returns the page the app already reads. v3.3.6 keeps one vector per Asset and orders by cosine distance too. Its column is a Postgres `vector`, and its index can drop a closer Asset. This server walks the blobs, so that Asset stays in the ranking.

## Answer

One CLIP vector per Asset is a row in SQLite. The value is a blob of float32 values. Its length is the length the ML service returned for the configured model. The default model returns 512 floats. Deleting the Asset deletes its CLIP vector. Trashing the Asset leaves the vector in place.

The server keeps that blob. The vector for a search string lasts for that search: the ML service returns it when the search runs, and it is not written. A similar-photos search loads the Asset's blob.

The server ranks the blobs by exact cosine distance, then asset id, and returns the page the app already reads. An Asset with no blob stays off that page. A similar-photos search whose Asset has no blob is the 400 named in [Inventory where smart-search embeddings live](19-inventory-where-smart-search-embeddings-live.md). An empty set is that note's empty 200 page.

A change of CLIP model is [Decide whether a CLIP model change clears stored vectors](35-decide-whether-a-clip-model-change-clears-stored-vectors.md). Whether an edited still gets a CLIP vector stays [Decide whether an edited still gets a CLIP vector](31-decide-whether-an-edited-still-gets-a-clip-vector.md).
