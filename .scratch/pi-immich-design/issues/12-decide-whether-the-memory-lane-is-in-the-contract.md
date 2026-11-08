# Decide whether the memory lane is in the Contract

Type: grilling
Status: resolved

## Question

The photos page mounts a memory lane from local rows that `POST /api/sync/stream` writes (`MemoriesV2` and `MemoryToAssetsV2`). That lane is not the timeline grid. [Inventory the timeline](03-inventory-the-timeline.md) records the mechanism. The success path names viewing the timeline. Is the memory lane part of the Contract, or can this server omit those sync types and leave the lane empty?

## Comments

Recommendation accepted: the memory lane is in the Contract. The server generates the rows and syncs `MemoriesV2` and `MemoryToAssetsV2`.

Recommendation accepted: generate both on-this-day and birthday cards.

Recommendation accepted: match the memory behavior the official app can see. The lane, the library memories card, the memories list, and opening a card follow v3.3.6. `memories.enabled` defaults to true and hides those surfaces when false. The memory HTTP routes stay out. The eight Pi constraints stay as [Decide which Pi constraints the app can see](08-decide-which-pi-constraints-the-app-can-see.md) recorded them.

Recommendation accepted: keep the v3.3.6 cleanup. An unsaved memory is deleted 30 days after it was created. An asset that leaves the timeline is removed from its memories. This app never sets `isSaved`.

## Answer

The memory lane is in the Contract, together with the library memories card, the memories list, and opening a card. Those surfaces read local rows from `MemoriesV2` and `MemoryToAssetsV2`. The server generates the rows. The app never calls the memory HTTP routes, and those routes are not in the Contract.

A Memory belongs to one User. It is an on-this-day card or a birthday card.

Generation runs nightly. The default is on, matching `nightlyTasks.generateMemories`. Each run fills the calendar days from three days ago through three days ahead, in UTC, and skips a day it has already filled. A new server waits for that nightly run. Upload does not create a card.

An on-this-day card is one per earlier year. It holds up to 20 of that User's timeline assets whose local date is that month and day, newest first. Each asset has a preview and is not deleted. The lane shows the card on that calendar day. `showAt` is the start of the day and `hideAt` is the end of the day.

A birthday card is one per named person who is not hidden, has a birthday, and was born in an earlier year. The job creates it for the birthday's calendar day. In a non-leap year, a 29 February birthday is taken on 28 February. The lane shows the card from three days before that day through the end of the day. The assets are that person's visible faces on timeline assets from their birthday in earlier years, with a preview and not deleted. The card holds at most 25 assets, and at most five from one year. The per-year count is the minimum of 5 and `floor(25 / number of years)`. Past 25 years, it keeps 25 of those years and one asset from each.

The lane shows cards whose window contains today and that still have a timeline asset. An empty lane draws nothing, and the timeline remains. The library card and the memories list show that User's memories, including cards outside today's window, until cleanup. The list's saved filter reads `isSaved`. `memories.enabled` defaults to true. When the synced preference is false, the lane, the library card, and the list stay empty.

Nightly cleanup removes an asset from a memory when that asset leaves the timeline. It deletes a memory 30 days after creation when `isSaved` is false. The app stores `isSaved` and `seenAt` from sync and never writes them, so the heart stays empty and the 30-day delete applies.

Opening a card uses the assets already on the memory. Thumbnail and Original requests stay the ones [Inventory thumbnails and originals](04-inventory-thumbnails-and-originals.md) recorded.

SQLite instead of Postgres is invisible. The app sees these sync rows.
