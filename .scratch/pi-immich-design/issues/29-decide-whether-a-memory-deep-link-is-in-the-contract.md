# Decide whether a memory deep link is in the Contract

Type: grilling
Status: resolved

## Question

The lane and the memories list open a card with the rows they already loaded. A link whose host is `memory` is a second way in. With no id it loads the lane. With an id it loads that memory through `MemoryRepository.get`, and that query throws when a linked asset is missing, trashed, or not on the timeline, even if another asset on the memory is visible. [Decide whether the memory lane is in the Contract](12-decide-whether-the-memory-lane-is-in-the-contract.md) puts opening a card in the Contract and does not name this link. [Inventory which memory fields the app stores](21-inventory-which-memory-fields-the-app-stores.md) records the query. Is the deep link part of opening a card?

## Comments

Recommendation accepted: the `immich://memory` link stays outside the Contract. Opening a card stays the handoff [Decide whether the memory lane is in the Contract](12-decide-whether-the-memory-lane-is-in-the-contract.md) locked. The lane and the memories list pass the rows they already loaded.

## Answer

The memory deep link stays outside the Contract. Opening a card is the lane and the memories list handing over the rows they already loaded, as [Decide whether the memory lane is in the Contract](12-decide-whether-the-memory-lane-is-in-the-contract.md) locked.

`immich://memory` and `immich://memory?id=` are that link. Nothing in the app, the home-screen widgets, or the server builds either URL. The handler and its test are the only references. With no id the handler would run the lane query. With an id it would run `MemoryRepository.get`, which throws when any linked asset is missing, trashed, or off the timeline. Both stay outside the Contract. The server grows no route and no second memory shape for the link. `my.immich.app` links do not point at a memory.
