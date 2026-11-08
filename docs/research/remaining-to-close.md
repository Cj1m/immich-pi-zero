# Remaining to close

What v3.3.6 still requires before the Pi Immich design map can leave its fog. Read from `reference/immich` at tag `v3.3.6`, commit `587f549f46f1cd1822a06e16dcccecef04ea4565`. This note does not decide the Pi behavior. It names the calls, columns, and screens a later ticket can decide.

The map's four fog patches are now sharp enough to ticket. They close in this order: the admin config document, the user-admin writes, the editor and face writes, the reopened web features, then the SQLite columns those bodies read. The photo shell (library, albums, trash, search, people, map) mostly reuses endpoints the mobile Contract already named.

## Admin config, the machine-learning URL, and object storage

The stock web still loads and saves the deprecated config routes. `systemConfigManager.init` calls `getConfig` and `getConfigDefaults` (`reference/immich/web/src/lib/managers/system-config-manager.svelte.ts:15-18`, `48-53`). Those SDK functions accept only 200 from `GET /api/system-config` and `GET /api/system-config/defaults` (`reference/immich/packages/sdk/src/fetch-client.ts:7258-7290`). `GET` and `PUT /api/admin/config` exist and return the same `AdminConfigDto` (`reference/immich/server/src/controllers/config-admin.controller.ts:14-45`). The web does not call them. `GET` and `PUT /api/system-config` were deprecated in v3.2.0 (`reference/immich/server/src/controllers/system-config.controller.ts:23-27`, `38-42`, `53-57`).

Every admin page waits on both GETs in `admin/+layout.ts`. The user list does not read the body. Opening a settings accordion reads one top-level key. A missing key throws on that first property (`MachineLearningSettings.svelte` reads `machineLearning.urls`).

Save is a full document. `handleSystemConfigSave` refetches the config, shallow-merges the edited top-level keys, and `PUT`s the whole `AdminConfigDto` (`reference/immich/web/src/lib/services/system-config.service.ts:47-57`). The machine-learning form saves the key `machineLearning` (`MachineLearningSettings.svelte`). The server stores only the keys that differ from defaults, under system metadata key `system-config` (`reference/immich/server/src/utils/config.ts`). `GET /api/system-config/defaults` is the in-process defaults object (`reference/immich/server/src/dtos/config.dto.ts:546-775`). With no saved overrides, the two bodies match, except `machineLearning.enabled` and `machineLearning.urls`, which are fixed from the environment when the module loads.

`machineLearning` defaults (`config.dto.ts:619-649`):

- `enabled`: true unless `IMMICH_MACHINE_LEARNING_ENABLED` is the string `false`
- `urls`: `[IMMICH_MACHINE_LEARNING_URL]` or `["http://immich-machine-learning:3003"]`
- `availabilityChecks`: enabled, timeout 2000, interval 30000
- `clip`: enabled, model `ViT-B-32__openai`
- `duplicateDetection`: enabled, `maxDistance` 0.01
- `facialRecognition`: enabled, model `buffalo_l`, `minScore` 0.7, `maxDistance` 0.5, `minFaces` 3
- `ocr`: enabled, model `PP-OCRv5_mobile`, `minDetectionScore` 0.5, `minRecognitionScore` 0.8, `maxResolution` 736

The form also edits those nested fields. CLIP, face, and OCR model names are strings on this object. A saved `machineLearning.urls` overrides the environment variable. `IMMICH_CONFIG_FILE` makes the form read-only and makes `PUT` return 400 `Cannot update configuration while IMMICH_CONFIG_FILE is in use`.

Auth on these four routes is an admin session. A missing session is 401 `Authentication required`. A non-admin is 403 `Forbidden`. There is no `DELETE`. Reset copies the defaults into the form and saves with the same `PUT`.

`GET /api/server/config`, `GET /api/config`, and `GET /api/public/config` are different documents.

v3.3.6 has no object-storage credentials and no credential form. `EnvSchema` has no bucket, region, endpoint, access key, or secret (`reference/immich/server/src/dtos/env.dto.ts:37-89`). Originals are files under `IMMICH_MEDIA_LOCATION` (`env.dto.ts:52`). If that variable is unset, the server picks `/data` or `/usr/src/app/upload` (`reference/immich/server/src/services/storage.service.ts:23-44`). Upload writes `{mediaLocation}/upload/{userId}/{aa}/{bb}/{id}{ext}`. The storage-template form edits `storageTemplate.enabled`, `template`, and `hashVerificationEnabled`, and it `GET`s `/api/system-config/storage-template-options`. The library form edits `library.scan` and `library.watch`. Those are local paths.

**To close this patch:** decide that `GET` and `PUT /api/system-config` and `GET /api/system-config/defaults` return this `AdminConfigDto`, because the settings page reads it and every save sends the whole document. Decide separately that object-storage credentials stay outside that document. The web has no field for them. The Pi still stores Originals in object storage; the operator sets those credentials where the v3.3.6 UI never looks.

## User change, delete, restore, and reset

The list context menu and `/admin/users/{id}/edit` call `user-admin.service.ts`. The client accepts only 200 and a `UserAdminResponseDto`.

- `PUT /api/admin/users/{id}` with `UserAdminUpdateDto`: optional `email`, `password`, `pinCode`, `name`, `avatarColor`, `storageLabel`, `shouldChangePassword`, `quotaSizeInBytes`, `isAdmin` (`reference/immich/server/src/dtos/user.dto.ts:95-109`, `user-admin.controller.ts:59-76`, `fetch-client.ts:4259-4270`).
- Password reset is that same `PUT` with a password the browser generates and `shouldChangePassword: true` (`user-admin.service.ts:171-184`). There is no separate reset route.
- PIN reset is that same `PUT` with `{ pinCode: null }` (`user-admin.service.ts:199`).
- `DELETE /api/admin/users/{id}` with `{ force?: boolean }` returns 200 and the user (`user.dto.ts:111-117`, `user-admin.controller.ts:89-101`, `fetch-client.ts:4230-4241`).
- `POST /api/admin/users/{id}/restore` returns 200 (`user-admin.controller.ts:186-195`, `fetch-client.ts:4324-4333`).

`deletedAt` on the user row is what restore clears. `pinCode` is the column the PIN reset writes.

**To close this patch:** decide that these three calls match v3.3.6. The create, detail, preferences, sessions, statistics, and heatmap calls are already decided.

## The editor and face boxes

The photo editor reads `GET /api/assets/{id}/edits`, then saves with `PUT /api/assets/{id}/edits` or `DELETE /api/assets/{id}/edits` when the edit list is empty (`reference/immich/web/src/lib/managers/edit/edit-manager.svelte.ts:117-141`, `EditorPanel.svelte`). Those bodies are already decided. The page also waits up to 10 seconds for the websocket event `AssetEditReadyV2` for that asset before it treats the save as done (`edit-manager.svelte.ts:129-141`). The mobile timeline already consumes that same event.

The face editor draws boxes in the browser from the face list. It does not need a new image response to draw them. It calls `GET /api/people` (`getAllPeople`) and `POST /api/faces` (`FaceEditor.svelte:11`, `149`; `face.controller.ts:22-31`). Reassignment is below.

**To close this patch:** decide that a saved edit is not finished for the web until `AssetEditReadyV2` arrives, and that `POST /api/faces` matches v3.3.6. The box the canvas draws is the face box the Contract already stores.

## Reopened features

Each of these is one match-or-not decision. The mobile app already writes asset favorite. Person favorite is a different call.

### Asset favorite, already on the phone

`FavoriteAction` calls `assetService.update` with `isFavorite` (`reference/immich/mobile/lib/presentation/actions/favorite.action.dart:54-55`). That is `PUT /api/assets` with `AssetBulkUpdateDto`, operation `updateAssets`, status 204 (`asset.controller.ts:56-69`, `open-api/immich-openapi-specs.json:3728-3878`). That `PUT` is marked deprecated in v3. The web heart uses `PUT /api/assets/{id}` with `{ isFavorite }` and accepts 200 `AssetResponseDto` (`reference/immich/web/src/lib/services/asset.service.ts:447-459`, `fetch-client.ts:4877-4888`).

**To close:** name the bulk `PUT /api/assets` on the mobile success path, and the single-asset `PUT` for the web heart. Both write `asset.isFavorite`.

### Person hide, person favorite, merge, and face reassignment

The web people page calls `PUT /api/people/{id}` with `{ isHidden: true }`, `{ isHidden: false }`, `{ isFavorite: true }`, or `{ isFavorite: false }` (`reference/immich/web/src/lib/services/person.service.ts:85-134`). The client accepts 200 `PersonResponseDto` (`fetch-client.ts:6278-6288`). Columns are `person.isHidden` and `person.isFavorite` (`person.table.ts:68-78`).

Merge is `POST /api/people/merge` with `{ ids }`, status 200, body `BulkIdResponseDto[]` (`person.controller.ts:196-206`, `MergeFaceSelector.svelte:70`, `fetch-client.ts:6190-6200`).

Bulk reassignment is `PUT /api/people/{id}/reassign`, status 200, `PersonResponseDto[]` (`person.controller.ts:181-193`, `fetch-client.ts:6310-6320`). One face moves with `PUT /api/faces/{id}` (`face.controller.ts:45-57`). Delete is `DELETE /api/faces/{id}`, status 204.

The mobile people screen stores these flags from sync and does not send these writes. That is why they were set aside, and why the web reopens them.

### Partner sharing

`GET /api/partners?direction=`, `POST /api/partners` (201), `PUT /api/partners/{id}`, `DELETE /api/partners/{id}` (`partner.controller.ts:16-73`, `fetch-client.ts:6044-6080`). The mobile timeline already shows partner assets that arrived on the sync stream. These routes are the web sharing screen that creates that relationship.

### Duplicate detection

`GET /api/duplicates` returns 200 `DuplicateResponseDto[]` (`duplicate.controller.ts:17-25`, `fetch-client.ts:5453-5459`). Dismiss is `DELETE /api/duplicates/{id}` (204). Resolve is `POST /api/duplicates/resolve` (200). The grouping uses `machineLearning.duplicateDetection` and the stored CLIP vector. It does not run a separate model on the Pi.

### Web OAuth

The login page, when public config says `oauth.enabled`, calls `POST /api/oauth/authorize` with `{ redirectUri }` equal to the page URL before the query string. The client accepts 201 and reads `url`, then navigates the browser there (`reference/immich/web/src/lib/utils.ts:348-358`, `fetch-client.ts:5967-5977`, `auth/login/+page.svelte:40-70`). The return with `code` or `error` calls `POST /api/oauth/callback` with `{ url: location.href }` and accepts 201 `LoginResponseDto` (`utils.ts:360-362`, `fetch-client.ts:5994-6004`). Account linking is `POST /api/oauth/link` (200) and `POST /api/oauth/unlink` (200).

The mobile password path already has the same two calls, with redirect URI `app.immich:///oauth-callback`. The issuer, client id, and client secret are `oauth.*` inside the admin config document (`config.dto.ts` defaults around the `oauth` key). Session columns `oauthSid` and `oauthBearerToken` exist for this login (`session.table.ts:56-60`). User column `oauthId` exists (`user.table.ts:60-61`).

### Memory HTTP routes

The memories page searches with `GET /api/memories` and accepts 200 `MemoryResponseDto[]` (`memory-manager.svelte.ts:340`, `fetch-client.ts:5733-5759`). Saving a card sends `PUT /api/memories/{id}` with `{ isSaved }` and accepts 200 (`memory-manager.svelte.ts:256-265`, `memory.controller.ts:69-85`). Removing a card sends `DELETE /api/memories/{id}`. The server hard-deletes the row (`memory.repository.ts:212-214`). The client uses `fetchText` and expects no JSON body (`fetch-client.ts:5810-5816`). The controller sets 204 (`memory.controller.ts:99-108`).

`PUT` can also store `seenAt` (`memory.service.ts:195-202`). The web page never sends `seenAt`. `showAt` and `hideAt` are columns (`memory.table.ts:60-64`) and are set on create, which this page does not call. So the web writer for `isSaved` is this `PUT`. The web writer for removal is a hard delete, which leaves `deletedAt` unset. `seenAt` still has no writer on this page.

## SQLite columns

v3.3.6 is Postgres. The Pi decision is SQLite. Closing this patch is naming the columns the decided bodies read. Audit tables, plugins, workflows, geodata, notifications, and tags can wait until a screen that reads them is decided.

Columns the decided shared-link bodies read (`shared-link.table.ts`, `shared-link-asset.table.ts`):

- `shared_link`: `id`, `description`, `userId`, `key` (bytea), `type`, `createdAt`, `expiresAt`, `allowUpload`, `albumId`, `allowDownload`, `showExif`, `password`, `slug`. The API name for `showExif` is `showMetadata`.
- `shared_link_asset`: `assetId`, `sharedLinkId`.

Columns the decided asset, timeline, download, and rendition bodies read (`asset.table.ts`, `asset-exif.table.ts`):

- `asset`: `id`, `ownerId`, `type`, `originalPath`, `originalFileName`, `fileCreatedAt`, `fileModifiedAt`, `localDateTime`, `isFavorite`, `duration`, `checksum`, `checksumAlgorithm`, `livePhotoVideoId`, `thumbhash`, `libraryId`, `deletedAt`, `stackId`, `visibility`, `width`, `height`, `isEdited`, `status`, `createdAt`, `updatedAt`, `updateId`. `isOffline`, `isExternal`, and `duplicateId` are on the same row. `duplicateId` is the duplicate-detection group.
- `asset_exif`: `fileSizeInByte` is the download size. Timeline and the detail panel also read `city`, `state`, `country`, `latitude`, `longitude`, `projectionType`, `description`, `rating`, `make`, `model`, `dateTimeOriginal`, `timeZone`, `orientation`, `exifImageWidth`, `exifImageHeight`. The rest of the exif row (`lensModel`, `fNumber`, `focalLength`, `iso`, `fps`, `exposureTime`, `livePhotoCID`, `profileDescription`, `colorspace`, `bitsPerSample`, `autoStackId`, `tags`, `lockedProperties`) is the detail panel.
- `asset_file` thumbnail, preview, and `{id}_fullsize_edited.jpeg` are already named. `encoded_video` is already named and stays empty on the playback path.

User and session columns the decided auth and admin responses read (`user.table.ts`, `session.table.ts`):

- `user`: `id`, `email`, `password`, `name`, `isAdmin`, `shouldChangePassword`, `quotaSizeInBytes`, `quotaUsageInBytes`, `avatarColor`, `storageLabel`, `status`, `profileImagePath`, `deletedAt`, `pinCode`, `createdAt`, `updatedAt`, `updateId`, `profileChangedAt`. User onboarding is not a column on `user`. It is user metadata key `Onboarding` (`user.service.ts:208-238`). Server `isOnboarded` is system metadata key `AdminOnboarding`.
- `session`: `id`, `token` (bytea, the SHA-256 the Contract stores), `userId`, `deviceType`, `deviceOS`, `appVersion`, `createdAt`, `updatedAt`, `expiresAt`, `parentId`, `updateId`, `isPendingSyncReset`, `pinExpiresAt`. `oauthSid` and `oauthBearerToken` wait on the OAuth decision.
- `system_metadata`: `key`, `value` jsonb. This row holds the admin config diff, admin onboarding, and the detected media location.

`memory` columns are `id`, `createdAt`, `updatedAt`, `deletedAt`, `ownerId`, `type`, `data`, `isSaved`, `memoryAt`, `seenAt`, `showAt`, `hideAt`, `updateId`. The link table is `memoryId` and `assetId`. The memory decision already named the row. `showAt`, `hideAt`, and `memoryAt` are the columns the search response still has to pin down when the memory HTTP ticket lands. `person` columns beyond name and thumbnail path are `isHidden`, `isFavorite`, `birthDate`, `faceAssetId`, `color`, `ownerId`, `personGroupId`, `updateId`.

**To close this patch:** one ticket for `shared_link` and `shared_link_asset`, one for `asset`, `asset_exif`, and `asset_file`, one for `user`, `session`, and `system_metadata`. Person, partner, duplicate, and memory columns wait on the feature tickets above. Postgres `bytea`, `jsonb`, `uuid`, and `timestamptz` become SQLite blob, text, text, and text. The CLIP vector is already a float32 blob. That storage choice does not change these columns.

## What can stay fog until those tickets land

The photo routes under `(user)/` — photos, archive, favorites, folders, trash, albums, people, places, search, map, tags, explore, locked, memories — share one timeline shell. Their extra calls are filters on endpoints the Contract already names (`visibility`, `isFavorite`, `personId`, album id). Inventorying that shell is one ticket after the writes above, and it may add no new response.

Admin jobs, queues, maintenance restore, integrity reports, ffmpeg settings, SMTP, and the purchase page are screens. Ffmpeg and video transcode stay out of scope because the Pi cannot run them. The settings page still has to return those keys inside the admin config document, or the accordion throws when an operator opens it.
