# Decide which memory HTTP responses this server matches

Type: grilling
Status: resolved

## Question

[Decide which memory fields this server stores](28-decide-which-memory-fields-this-server-stores.md) leaves `isSaved` false and `seenAt` and `deletedAt` null until a memory HTTP route writes them. The web memories page calls `GET /api/memories`, `PUT /api/memories/{id}` with `isSaved`, and `DELETE /api/memories/{id}`. The mobile Contract does not call those routes. The call shapes are in [Remaining to close](../../../docs/research/remaining-to-close.md). Where the web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do those three calls match v3.3.6?

## Comments

Recommendation accepted: the three calls match v3.3.6. The same page also asks for the count and removes one Asset. `PUT` is the writer for `isSaved`. `DELETE` hard-deletes the row.

## Answer

The three calls match v3.3.6. The phone does not send them. It stores the sync that follows. There is no Pi difference.

`GET /api/memories` returns 200 and the memory documents that still have at least one Asset. A document is `id`, `createdAt`, `updatedAt`, `deletedAt`, `memoryAt`, `seenAt`, `showAt`, `hideAt`, `ownerId`, `type`, `data`, `isSaved`, and `assets`. `type` and `data` stay as [Decide which memory fields this server stores](28-decide-which-memory-fields-this-server-stores.md). `showAt` and `hideAt` are the window generation already sets. Each asset is the asset document. The lane, the explore card, and the list read `id`, `type`, `data.year`, `data.personName`, `isSaved`, `memoryAt`, and the assets. The list asks with `size` 250 and `order` `desc`. Explore asks with `size` 12. The library lane sends `for` as today's date. A saved-only list sends `isSaved=true`. `for` keeps a memory whose `showAt` is null or on or before that date, and whose `hideAt` is null or on or after that date. Order `desc` is `showAt` descending, nulls last, then `memoryAt` descending. `page` with `size` is the offset. Assets on a memory are timeline visibility, not deleted, and not a face of a hidden person, oldest `fileCreatedAt` first. A memory with no such Asset is left off the list. The owner is the caller. `deletedAt` must be null unless `isTrashed` is sent. The page does not send `isTrashed`.

`GET /api/memories/statistics` with the same query returns 200 `{ total }`. The page reads `total`.

`PUT /api/memories/{id}` with `{ isSaved }` returns 200 and the memory document. The page reads no field and sets `isSaved` locally. That is the writer for `isSaved`. The next sync sends it on `MemoriesV2`. The body may also include `seenAt` or `memoryAt`, and those are stored. This page sends neither, so `seenAt` stays null. A missing memory is 400 `Not found or no memory.update access`. An empty body is 400.

`DELETE /api/memories/{id}` returns 204 and an empty body. The row is removed. `deletedAt` stays unset. The next sync sends `MemoryDeleteV1` and a `MemoryToAssetDeleteV1` for each Asset that was on it, as ticket 28 decided. A missing memory is 400 `Not found or no memory.delete access`.

Removing one Asset is `DELETE /api/memories/{id}/assets` with `{ ids }`. The response is 200 and one `{ id, success, error? }` per id. The page reads no field. The pair is dropped and syncs `MemoryToAssetDeleteV1`. When that Asset is the last one, the page sends the memory delete instead.

An unsigned call is 401. A shared-link session is 403 `Forbidden`.

`PATCH /api/memories/{id}` is the same update as the PUT. The web sends the PUT.
