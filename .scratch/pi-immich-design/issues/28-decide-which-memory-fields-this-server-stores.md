# Decide which memory fields this server stores

Type: grilling
Status: resolved

## Question

[Inventory which memory fields the app stores](21-inventory-which-memory-fields-the-app-stores.md) names what the v3.3.6 app writes from `MemoriesV2` and `MemoryToAssetsV2`, and what a missing field does. The app stores `id`, `createdAt`, `deletedAt`, `ownerId`, `type`, `data`, `isSaved`, `memoryAt`, `seenAt`, `showAt`, and `hideAt`. It does not store the server's `updatedAt`. `seenAt` is stored and no screen reads it. `personId` stays inside `data` and the title does not read it. A null `seenAt`, `showAt`, or `hideAt` does not clear a value the app already stored. A missing required non-null field fails that sync batch and does not ack it. `MemoryDeleteV1` and `MemoryToAssetDeleteV1` are the delete lines. Which of these fields does this server keep, and which does the sync line include?

## Comments

Recommendation accepted: this server keeps the `MemoriesV2` row v3.3.6 selects — `id`, `createdAt`, `updatedAt`, `deletedAt`, `ownerId`, `type`, `data`, `isSaved`, `memoryAt`, `seenAt`, `showAt`, and `hideAt` — and sends that row. `isSaved` stays false. `seenAt` and `deletedAt` stay null until something sets them. `data` is `{ year }` for an on-this-day card and `{ year, personId, personName }` for a birthday card.

Recommendation accepted: the asset link is the pair `memoryId` and `assetId` only. `MemoryToAssetsV2` sends that pair. An asset that leaves the timeline drops the pair and syncs `MemoryToAssetDeleteV1`. Deleting the memory syncs `MemoryDeleteV1` and a `MemoryToAssetDeleteV1` for each asset that was on it. Neither delete removes the Asset.

Recommendation accepted: `isSaved`, `seenAt`, and `deletedAt` stay false, null, and null. Nothing on this server writes them until a later ticket gives the memory HTTP routes a writer. The columns stay on the row and on `MemoriesV2`. Cleanup deletes the row and does not fill `deletedAt`.

## Answer

This server keeps one memory row per card and sends that row as `MemoriesV2`. The columns are `id`, `createdAt`, `updatedAt`, `deletedAt`, `ownerId`, `type`, `data`, `isSaved`, `memoryAt`, `seenAt`, `showAt`, and `hideAt`. `updatedAt` is on the wire, and the app drops it. Change discovery, including `updateId`, stays with the SQLite shape.

`type` is `on_this_day` or `birthday`. `data` is `{ year }` for an on-this-day card. A birthday card is `{ year, personId, personName }`, and `personId` is that person's `personGroupId`. `showAt` and `hideAt` are the window [Decide whether the memory lane is in the Contract](12-decide-whether-the-memory-lane-is-in-the-contract.md) already set. A later null `seenAt`, `showAt`, or `hideAt` does not clear the value the app already stored. Generation sets `showAt` and `hideAt`, and this server has no writer that clears them.

`isSaved` stays false. `seenAt` stays null. `deletedAt` stays null. Saving a memory, marking it seen, and soft-deleting it wait until a later ticket gives the memory HTTP routes a writer. Cleanup hard-deletes the row. It does not fill `deletedAt`.

Each asset on the card is the pair `memoryId` and `assetId`. The pair has no position and no timestamps. `MemoryToAssetsV2` sends the two ids. The app orders those assets by the asset's own `createdAt`.

An asset that leaves the timeline drops its pair. That syncs `MemoryToAssetDeleteV1` as `{ memoryId, assetId }`. Deleting the memory syncs `MemoryDeleteV1` as `{ memoryId }` and a `MemoryToAssetDeleteV1` for each asset that was on it. The Asset stays.

A missing required non-null key still fails that sync batch and is not acked, as [Inventory which memory fields the app stores](21-inventory-which-memory-fields-the-app-stores.md) recorded.
