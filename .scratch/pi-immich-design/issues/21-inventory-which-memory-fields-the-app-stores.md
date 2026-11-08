# Inventory which memory fields the app stores

Type: research
Status: resolved

## Question

[Decide whether the memory lane is in the Contract](12-decide-whether-the-memory-lane-is-in-the-contract.md) puts the lane, the library memories card, the memories list, and opening a card in the Contract. The server syncs `MemoriesV2` and `MemoryToAssetsV2`. Which fields does the v3.3.6 app store from those lines, and which of those fields do the lane, the library card, the list, and the viewer read? Include the delete lines. Say what a missing field does to the screen. Write the note to `docs/research/memories.md` and link it from this ticket when resolving.

## Answer

The app stores `id`, `createdAt`, `deletedAt`, `ownerId`, `type`, `data`, `isSaved`, `memoryAt`, `seenAt`, `showAt`, and `hideAt` from `MemoriesV2`. It does not store the server's `updatedAt`. A null `seenAt`, `showAt`, or `hideAt` on an existing row leaves the previous value. `MemoryToAssetsV2` stores `memoryId` and `assetId` only. `MemoryDeleteV1` removes the memory and its asset pairs. `MemoryToAssetDeleteV1` removes one pair. Neither delete removes the Asset.

The lane uses `showAt`, `hideAt`, `memoryAt`, `type`, and `data` (`year`, `personName`). The library card uses `memoryAt` and the first asset. The list adds `isSaved` for the heart and the saved filter, and formats `memoryAt` for an on-this-day title. The viewer uses `type`, `data`, and the assets it was opened with. `seenAt` is stored and no screen reads it. `personId` stays inside `data` and the title does not read it.

A missing required non-null field, or a `data` object with no integer `year`, does not land as an empty card. The first fails the sync batch and does not ack it. The second fails the query: the lane draws nothing, and the library card and the list show the error body. A memory with no timeline asset is left out of all four screens.

The field list, the delete lines, and those screens are in [Memories](../../../docs/research/memories.md).
