# Memories

Which fields the v3.3.6 mobile app stores from `MemoriesV2` and `MemoryToAssetsV2`, and which of those fields the photos memory lane, the library memories card, the memories list, and the memory viewer read. Read from `reference/immich` at tag `v3.3.6`, commit `587f549f46f1cd1822a06e16dcccecef04ea4565` (`git describe --tags --exact-match HEAD` and `git rev-parse HEAD` both match). This note does not run the server or the app.

`mobile/generated/openapi` is not in the checkout, and `git ls-files` does not track it. `SyncMemoryV2.fromJson` is called, and its body is not a file here. The generator pinned for that client is openapi-generator `7.25.0` (`reference/immich/open-api/openapitools.json`). The project's patch of the model template removes the debug-only required-key asserts and calls `upgradeDto` before the parse (`reference/immich/open-api/templates/mobile/serialization/native/native_class.mustache.patch`). `upgradeDto` has no entry for `SyncMemoryV2`, `SyncMemoryDeleteV1`, `SyncMemoryAssetV1`, or `SyncMemoryAssetDeleteV1` (`reference/immich/mobile/lib/utils/openapi_patching.dart:18-48`). A sibling generated model that is patched in this tree uses `mapValueOfType<String>(json, r'id')!` for a required string (`reference/immich/open-api/patch/asset_edit_action_item_response_dto.dart.patch:12-13`). The field lists below are the OpenAPI schema. Where a missing key throws, that is this generator's `!` on a required non-null field, not a line in the missing generated file.

## Sync lines

Against a server at least `3.3.0`, the app asks for `MemoriesV2` and `MemoryToAssetsV2` (`reference/immich/mobile/lib/domain/models/server_capability.model.dart:16`, `reference/immich/mobile/lib/infrastructure/repositories/sync_api.repository.dart:60-61`). v3.3.6 takes that branch. Older servers ask for `MemoriesV1` and `MemoryToAssetsV1`. The handlers below are the same either way.

`MemoriesV2` is entity `MemoryV2`, schema `SyncMemoryV2` (`reference/immich/server/src/services/sync.service.ts:800-810`, `reference/immich/server/src/dtos/sync.dto.ts:308-310`, `514`). It extends `SyncMemoryV1` and replaces `type` with the full `MemoryType` (`sync.dto.ts:291-310`). `MemoryType` is `on_this_day` or `birthday` (`reference/immich/server/src/enum.ts:89-97`). Every key is required (`reference/immich/open-api/immich-openapi-specs.json:31691-31704`). `id` and `ownerId` are uuid strings. `type` is `on_this_day` or `birthday`. `data` is an object. `isSaved` is a boolean. `createdAt`, `updatedAt`, and `memoryAt` are date-times and are not null. `deletedAt`, `seenAt`, `showAt`, and `hideAt` are date-times and are null or a date-time.

`data` has no required keys of its own. The server writes `{ year }` for an on-this-day card and `{ personId, personName, year }` for a birthday card (`reference/immich/server/src/services/memory.service.ts:76`, `107`). The sync schema does not name those keys (`sync.dto.ts:299`).

The same request also sends `MemoryDeleteV1` before the upserts (`sync.service.ts:801-805`). That object is `{ memoryId }` and both the key and the uuid are required (`sync.dto.ts:312-314`).

`MemoryToAssetsV2` is entity `MemoryToAssetV2`. Its data schema is `SyncMemoryAssetV1`, the same object as `MemoryToAssetV1`: `{ memoryId, assetId }`, both uuid, both required (`sync.dto.ts:316-321`, `517`, `sync.service.ts:828-838`). The server selects `memory_asset.memoriesId` as `memoryId` and `memory_asset.assetId` (`reference/immich/server/src/repositories/sync.repository.ts:615-621`). `memory_asset.createdAt`, `updatedAt`, and `updateId` are not selected (`reference/immich/server/src/schema/tables/memory-asset.table.ts:30-36`). There is no position column.

That request also sends `MemoryToAssetDeleteV1` first (`sync.service.ts:829-833`). The object is `{ memoryId, assetId }`, both required (`sync.dto.ts:323-328`, `518`).

The app parses `MemoryV1` and `MemoryV2` with `SyncMemoryV2.fromJson`, and `MemoryToAssetV1` and `MemoryToAssetV2` with `SyncMemoryAssetV1.fromJson` (`sync_api.repository.dart:187-192`). `_parseLines` does not catch a throw from `fromJson` (`sync_api.repository.dart:124-138`). `streamChanges` returns that error and stops reading the stream (`sync_api.repository.dart:117-118`). `_processBatch` acks only after the handler returns (`reference/immich/mobile/lib/domain/services/sync_stream.service.dart:181-188`). A throw from `fromJson` happens before `onData`, so that chunk is not stored and the stream stops. A throw from the handler skips the ack for that entity batch. Lines already acked earlier stay. Lines not yet read are not applied. The four screens keep whatever rows the last acked batch left.

For a required non-null field, the generator emits a null-check after the mapper. A missing key, a JSON null, or a value of the wrong JSON type fails that check: `mapDateTime` returns null for a missing or unparseable date-time, `mapValueOfType<T>` returns null when the value is not a `T`, and `MemoryType.fromJson` returns null for any string other than the two enum values. `data` uses `mapCastOfType` and the same check, so a missing `data`, a JSON null, or a non-object fails the parse. The required nullable dates (`deletedAt`, `seenAt`, `showAt`, `hideAt`) do not get that check. A missing key and a JSON null both become a null `DateTime` and the parse continues. `SyncEvent.data` is non-null (`reference/immich/mobile/lib/domain/models/sync_event.model.dart:5`). A `fromJson` that returns null, which it does when `data` is not a Map, fails when the event is constructed. Same outcome: the batch is not stored and not acked.

`MemoryDeleteV1` and `MemoryToAssetDeleteV1` are the same shape. A missing `memoryId` or `assetId` fails the parse. The row they would have removed stays.

## Stored columns

`updateMemoriesV2` inserts `memory_entity` and, on `id` conflict, updates the same companion (`reference/immich/mobile/lib/infrastructure/repositories/sync_stream.repository.dart:625-646`). The columns are `id`, `createdAt`, `deletedAt`, `ownerId`, `type`, `data`, `isSaved`, `memoryAt`, `seenAt`, `showAt`, and `hideAt`.

`type` is stored as an int enum, `onThisDay` then `birthday`, in that order (`reference/immich/mobile/lib/domain/models/memory.model.dart:9-13`, `sync_stream.repository.dart:952-956`). `data` is `jsonEncode` of the whole object, not a picked subset (`sync_stream.repository.dart:634`). `personId` and any other key ride along inside that string. `seenAt`, `showAt`, and `hideAt` use `Value.absentIfNull`. A null does not write the column. On a new id the nullable column stays null. On an existing id a later null leaves the previous timestamp in place. A later non-null replaces it. `deletedAt` is written even when null, so a later line clears a soft delete.

`updatedAt` is required on the wire and is not in the companion. The column defaults to the current time (`reference/immich/mobile/lib/data/db/main/table/memory/memory.dart:14`). Insert stamps that default. Conflict update does not change it. The server's `updatedAt` is dropped. No screen reads the column.

The table is strict and without a row id (`reference/immich/mobile/lib/data/db/util/defaults_mixin.dart:8-9`, `memory.dart:34-35`). `ownerId` references `user_entity` with cascade delete (`memory.dart:18`). Foreign keys are on (`reference/immich/mobile/lib/data/db/main/database.dart:398`, `519`). An `ownerId` that is not a local user fails the insert. The handler rethrows (`sync_stream.repository.dart:648-651`), the batch does not commit, and the ack does not run.

`updateMemoryAssetsV1` inserts `(memoryId, assetId)` and does nothing on conflict (`sync_stream.repository.dart:668-674`). The primary key is those two columns (`reference/immich/mobile/lib/data/db/main/table/memory/asset.dart:9-14`). A repeated pair does not change the row. There is no other column to change. `memoryId` cascades from `memory_entity`. `assetId` cascades from `remote_asset_entity`. A pair whose memory row or asset row is missing fails the insert, rolls back that batch, and skips the ack.

Dates are clamped to year 1 through the end of year 9999 before they are stored (`reference/immich/mobile/lib/data/db/util/datetime_clamp_type.dart:4`, `27-37`). A date inside that range is stored as parsed. A date outside it is stored as the near edge. The screen then sees the clamped value.

## Deletes

`MemoryDeleteV1` deletes `memory_entity` where `id` equals `memoryId` (`sync_stream.repository.dart:655-660`). The memory-asset rows go with it, because `memoryId` is `ON DELETE CASCADE` (`asset.dart:11`). The asset row stays. An id that is not local deletes nothing and does not throw.

`MemoryToAssetDeleteV1` deletes the one `(memoryId, assetId)` row (`sync_stream.repository.dart:683-691`). The memory row and the asset row stay. An unknown pair deletes nothing and does not throw.

An asset row that disappears takes its memory-asset rows with it, by the asset foreign key (`asset.dart:9`). The memory row stays. The screens below also drop an asset whose `deletedAt` is set or whose visibility is not timeline, without waiting for a delete line.

None of the four screens watches the table. The lane reloads when the Photos tab is selected (`reference/immich/mobile/lib/pages/common/tab_shell.page.dart:113-114`) and when the app resumes after remote sync (`reference/immich/mobile/lib/providers/app_life_cycle.provider.dart:140-145`). The library card and the list use `allMemoriesProvider`, which that resume invalidates and the Photos tab does not. The viewer is given the list it was opened with. A delete while it is open does not remove a card or pop the route (`reference/immich/mobile/lib/presentation/pages/memory.page.dart:23-26`).

After a reload, a removed memory is gone from the lane, the card's grid, and the list. The lane draws nothing when the list is empty (`reference/immich/mobile/lib/presentation/widgets/memory/memory_lane.widget.dart:20-23`). The library card and the list keep their chrome and show an empty grid. The timeline is still there.

## What the screens share

The lane and the list load through `MemoryRepository.getAll` (`reference/immich/mobile/lib/domain/services/memory.service.dart:10-16`, `reference/immich/mobile/lib/infrastructure/repositories/memory.repository.dart:15-58`). The query keeps a memory only when `ownerId` is the signed-in user, `deletedAt` is null, and at least one linked asset has `deletedAt` null and visibility timeline. Those joins are inner. A memory with no such asset is absent. It is not shown empty.

Asset order inside a memory is `remote_asset.createdAt` ascending. Memory order is `memoryAt` descending (`memory.repository.dart:41`). The sync line has no order field.

`MemoryData.fromJson` keeps `year` and `personName` (`memory.model.dart:25-27`). `year` must be an int. A missing `year`, a null, or any other JSON type throws there, while `toDto` is building the list (`memory.repository.dart:98-114`). One such row fails the whole query. `personId` is not read. A `personName` that is not a string and not null throws the same way. Extra keys stay in the stored text and are ignored.

`memories.enabled` is not a memory column. It is `preferences.memories.enabled` on the user's metadata, and a missing value is true (`reference/immich/mobile/lib/infrastructure/utils/user.converter.dart:25`, `reference/immich/mobile/lib/infrastructure/repositories/user.repository.dart:61-66`). When it is false, both providers return an empty list before the query (`reference/immich/mobile/lib/providers/infrastructure/memory.provider.dart:9-14`, `24-28`).

## Lane

`memoryLaneProvider` calls `getAll` with `onlyToday` true, which is the default (`memory.provider.dart:21`, `memory.repository.dart:15`). Today is local midnight built as a UTC `DateTime` (`memory.repository.dart:33-38`). `showAt` null or `showAt <=` that midnight, and `hideAt` null or `hideAt >=` that midnight, keeps the row. A null pair is inside every day. The lane does not read `isSaved`, `seenAt`, `createdAt`, `updatedAt`, or `ownerId` except for that owner filter.

An empty result, a disabled preference, or a signed-out user is `SizedBox.shrink` (`memory_lane.widget.dart:20-23`). A first-load error has no value, so the lane also draws nothing. It does not show the error text. `memoryLane.value ?? const []` is how that happens (`memory_lane.widget.dart:19-20`).

Each card's picture is `assets[0].id` and `assets[0].thumbHash`, and a null thumbhash is passed as `""` (`memory_lane.widget.dart:79-82`). `assets[0]` is the oldest timeline asset, because of the `createdAt` order. The title is `MemoryTitle` with `preferDate` false (`memory_lane.widget.dart:89-91`). On this day, that is the translation `years_ago` of `DateTime.now().year - data.year` (`reference/immich/mobile/lib/presentation/widgets/memory/memory_title.widget.dart:10-16`). The lane does not format `memoryAt`. A birthday title uses `personName`. Null or empty is the translation `unknown` (`memory_title.widget.dart:19-23`). The lane does not pass the asset into the title, so the age line is not used here. A birthday also draws a cake icon (`memory_title.widget.dart:45-56`).

The tap still opens the viewer when the asset list is empty, and it only sets the current asset when the list is not (`memory_lane.widget.dart:25-30`). `getAll` does not return an empty asset list, so that branch is the query's inner join, not a card with a blank picture.

## Library card

The card is always in the library wrap (`reference/immich/mobile/lib/presentation/pages/library.page.dart:138-143`). It watches `allMemoriesProvider(false)` (`library.page.dart:344`). That is `onlyToday` false and `onlyFavorites` false, so `showAt`, `hideAt`, and `isSaved` do not filter it (`memory.service.dart:14-15`). A disabled preference or a signed-out user is an empty list (`memory.provider.dart:24-28`). The card still draws. The grid is empty and the label `memories` stays (`library.page.dart:368-397`). A first-load error uses `widgetWhen`, which shows `ScaffoldErrorBody` when there is no previous value (`reference/immich/mobile/lib/extensions/asyncvalue_extensions.dart:29-31`).

The grid is the first four memories, `memoryAt` descending, each cell `assets[0].id` and thumbhash (`library.page.dart:377-385`). It does not read the title, `data.year`, `personName`, `isSaved`, or `seenAt`. The tap opens the memories list, not a memory (`library.page.dart:353`).

## List

The list watches `allMemoriesProvider` with the favorites toggle (`reference/immich/mobile/lib/presentation/pages/memory_list.page.dart:23-27`, `35-39`). Off, the query is the library card's query. On, it adds `isSaved == true` (`memory.repository.dart:29-31`). `showAt` and `hideAt` are not applied. A saved memory with no timeline asset is absent, not shown as an empty favorite.

A cell's picture is `assets[0]`, same as the lane (`memory_list.page.dart:67-70`). The title uses `preferDate` true (`memory_list.page.dart:79-83`). On this day, that formats `memoryAt` with `yMMMMd` and does not use `data.year` (`memory_title.widget.dart:12-15`). A birthday still uses `personName`, and still falls back to `unknown`. The heart icon is drawn only when `isSaved` is true (`memory_list.page.dart:85-90`). `seenAt` is not read. The tap opens the viewer with this list and does not check that `assets` is empty (`memory_list.page.dart:56-58`). The inner join is what keeps `assets[0]` in range.

An empty result is an empty grid under the app bar. The favorites button stays. A first-load error is `ScaffoldErrorBody`, same as the card (`memory_list.page.dart:44-45`, `asyncvalue_extensions.dart:29-31`). One memory whose `data` has no integer `year` is that error for the whole list.

## Viewer

The lane and the list push `MemoryRoute` with the memories they already loaded (`memory_lane.widget.dart:30`, `memory_list.page.dart:58`). The page does not query again (`memory.page.dart:23-26`). It shows that list, then one epilogue page (`memory.page.dart:236-247`).

The picture is the asset at the horizontal index (`memory.page.dart:279-290`). A video asset draws the video viewer and a video icon (`reference/immich/mobile/lib/presentation/widgets/memory/memory_card.widget.dart:57-69`, `memory.page.dart:339-343`). Those come from the asset row. The memory line only supplied the id.

The title on the first asset is `MemoryTitle` (`memory_card.widget.dart:74-80`). The bottom line is `getMemoryTitle` with the current asset, plus that asset's local `createdAt` (`reference/immich/mobile/lib/presentation/widgets/memory/memory_bottom_info.widget.dart:19-36`). On this day, the title is `years_ago` from `data.year`, not `memoryAt`. A birthday with a `personName` and an asset whose local year is at least `data.year + 1` uses the age title. Age below 1, or no asset, uses the name without an age (`memory_title.widget.dart:19-30`). The viewer does not read `isSaved`, `seenAt`, `showAt`, `hideAt`, `createdAt`, or `updatedAt`.

`assets.length` is the progress denominator (`memory.page.dart:264-265`). The bottom info indexes `assets` (`memory.page.dart:351-355`). An empty asset list divides by zero and indexes an empty list. The lane and the list do not hand over that list.

A deep link with host `memory` is a second way in (`reference/immich/mobile/lib/services/deep_link.service.dart:49-50`). No id loads the lane query. An id calls `MemoryRepository.get` (`deep_link.service.dart:86-106`). `get` left-joins assets and then `readTable` on every joined asset (`memory.repository.dart:61-90`). A linked asset that is missing, trashed, or not timeline makes that read throw, so one such asset fails the open even when another asset on the memory is on the timeline. No row, or an empty result, returns null and the link opens nothing (`memory.repository.dart:78-79`, `deep_link.service.dart:102-104`). This path is not one of the four screens. It is the only caller of `get`.

## Writes

The app constructs `MemoriesApi` and does not call it. The only assignment is in `ApiService` (`reference/immich/mobile/lib/services/api.service.dart:40`, `77`). A search of `mobile/lib` finds no `memoriesApi` use and no write of `isSaved` or `seenAt` except the sync insert (`sync_stream.repository.dart:635-637`). The heart stays at the synced `isSaved`. Nothing in these screens sets `seenAt`.
