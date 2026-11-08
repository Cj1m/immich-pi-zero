# Shared link after open

Which HTTP calls the official web UI makes after a shared link is already open, for download, upload, slideshow, the map modal, comments, hover playback, and opening one Asset from the grid. Read from `reference/immich` at tag `v3.3.6`, commit `587f549f46f1cd1822a06e16dcccecef04ea4565`. This is the web UI on `/s/{slug}` and `/share/{key}`. The first paint is [Shared link open](shared-link-open.md). The mobile comments screen is [Album activity](album-activity.md).

The web SDK accepts only the status it declares (`oazapfts.ok`). Paths start with `/api` because the SDK `baseUrl` is `/api` (`reference/immich/packages/sdk/src/fetch-client.ts:11`). `createUrl` drops `undefined` and `null` (`reference/immich/web/src/lib/utils.ts:175-182`). On these routes `authManager.params` is `{ key, slug }` with the one the route does not have left `undefined` (`reference/immich/web/src/lib/managers/auth-manager.svelte.ts:18-19`).

## Download

The album toolbar downloads when `album.assetCount > 0` and `allowDownload` is true (`reference/immich/web/src/lib/components/album-page/AlbumViewer.svelte:129-144`). That click calls `handleDownloadAlbum`, which posts the album id (`reference/immich/web/src/lib/services/album.service.ts:375-376`). The individual toolbar, shown when `allowUpload` is true or there is more than one Asset, posts every id on the link when `allowDownload` is true (`reference/immich/web/src/lib/components/share-page/IndividualSharedViewer.svelte:46-48`, `79`, `126-134`). A multi-select bar uses `DownloadAction` on both pages under the same `allowDownload` check (`AlbumViewer.svelte:100-105`, `IndividualSharedViewer.svelte:99-101`). One selected Asset is a single-file download. More than one is the archive (`reference/immich/web/src/lib/components/timeline/actions/DownloadAction.svelte:21-31`).

The viewer download is `SharedLinkDownload`. It is shown when the signed-in User owns the Asset or `allowDownload` is true (`reference/immich/web/src/lib/services/asset.service.ts:176-179`, `reference/immich/web/src/lib/components/asset-viewer/AssetViewerNavBar.svelte:114`). The menu items `Download` and `DownloadOriginal` render only when there is no shared link in memory (`AssetViewerNavBar.svelte:124-129`).

## Upload

The add button is shown when `allowUpload` is true (`AlbumViewer.svelte:118-126`, `IndividualSharedViewer.svelte:115-123`). A file drop on a shared-link route sets the drag store, and both viewers upload those files (`reference/immich/web/src/routes/(user)/DragAndDropUploadOverlay.svelte:130-131`, `AlbumViewer.svelte:40-46`, `IndividualSharedViewer.svelte:37-44`). The drop handler does not read `allowUpload`. The album button passes `albumId`. The individual button passes no `albumId` (`IndividualSharedViewer.svelte:50-54`).

## Slideshow

The album toolbar starts a slideshow in the same `assetCount > 0 && allowDownload` block as its download button (`AlbumViewer.svelte:129-137`). An individual link has no slideshow button. A still that is already open also starts one when the recipient presses `s` (`reference/immich/web/src/lib/components/asset-viewer/PhotoViewer.svelte:115`, `212`). That key sets the local slideshow state. It does not request by itself.

## Map modal

The map button is mounted when `showMetadata` is true and `features.map` is true (`AlbumViewer.svelte:147-148`). `GET /api/albums/{id}/map-markers` already ran on that mount. The click opens the modal with the stored array (`reference/immich/web/src/lib/components/album-page/AlbumMap.svelte:53-60`). Choosing a marker navigates to that Asset, which is the open-one-Asset path below. Closing the viewer reopens the modal from the same array (`AlbumMap.svelte:32-38`).

## Comments

The shared-link album page passes the album into the timeline and does not pass `isShared` (`AlbumViewer.svelte:76`). The timeline default is false (`reference/immich/web/src/lib/components/timeline/Timeline.svelte:76`). The individual viewer does not pass an album. `activityManager.init` runs only when the viewer has an album, `isShared` is true, and the Asset has an id (`reference/immich/web/src/lib/components/asset-viewer/AssetViewer.svelte:386-389`). The activity panel also requires a signed-in user session (`AssetViewer.svelte:686`). These pages do not call `GET /api/activities`, `POST /api/activities`, `DELETE /api/activities/{id}`, or `GET /api/activities/statistics`.

Those four routes are the ones [Album activity](album-activity.md) and [Decide which activity fields this server stores](../../.scratch/pi-immich-design/issues/34-decide-which-activity-fields-this-server-stores.md) already match. The activity controller does not set `sharedLink: true` (`reference/immich/server/src/controllers/activity.controller.ts:23-24`, `35-36`, `54-55`, `65-66`). A shared-link session on a route that is not marked for it gets 403 `Forbidden` (`reference/immich/server/src/services/auth.service.ts:234-236`). The signed-in album page at `/albums/{id}` is a different screen.

## Hover playback

A video thumbnail sets a playback URL and gives it to the player when the pointer is over the tile, or over the play icon when the hover preference is off (`reference/immich/web/src/lib/components/assets/thumbnail/Thumbnail.svelte:255-264`, `reference/immich/web/src/lib/components/assets/thumbnail/VideoThumbnail.svelte:52-73`). The stored default of that preference is true (`reference/immich/web/src/lib/stores/preferences.store.ts:130`). A still with `livePhotoVideoId` uses that id in the same URL (`Thumbnail.svelte:266-277`). Hover does not read `realtimeTranscoding`. The playback route is the one [Shared link open](shared-link-open.md) already names: `GET /api/assets/{id}/video/playback` of the Original, status 200 or 206. `realtimeTranscoding` is false, so the opened viewer does not request HLS. Hover never requests it.

## Opening one Asset

A click on an album tile navigates to `/photos/{id}` under the current share URL (`Timeline.svelte:542-552`, `reference/immich/web/src/lib/utils/navigation.ts:38-48`, `109-115`). A click on an individual tile does the same (`reference/immich/web/src/lib/components/shared-components/gallery-viewer/GalleryViewer.svelte:355-360`). The page load calls `getAssetInfoFromParam` (`reference/immich/web/src/lib/utils/shared-links.ts:33-40`). The user layout then opens the viewer with that Asset (`reference/immich/web/src/routes/(user)/+layout.svelte:14-18`). The shared link stays the in-memory copy, so this navigation does not call `GET /api/shared-links/me` again.

An album viewer then loads the next and previous Assets (`reference/immich/web/src/lib/components/timeline/TimelineAssetViewer.svelte:42-86`). An individual viewer takes next and previous from the `assets` already on the link (`GalleryViewer.svelte:334-337`, `reference/immich/web/src/lib/utils/asset-utils.ts:385-392`). OCR, faces, and the stack request are skipped while a shared link is in memory (`AssetViewer.svelte:121-123`, `398-409`). The detail panel does not call `GET /api/albums` (`reference/immich/web/src/lib/components/asset-viewer/DetailPanel.svelte:59-62`).

## Calls

### `POST /api/download/info?key=` or `?slug=`

- When: an album download, an individual download of the whole link, or a multi-select of more than one Asset.
- Method and path: `POST /api/download/info` (`fetch-client.ts:5421-5436`). The client accepts only 201. The route allows a share key (`reference/immich/server/src/controllers/download.controller.ts:16-26`).
- Request: JSON. An album sends `albumId`. The other two send `assetIds`. `archiveSize` is added only when a user session is authenticated, from `preferences.download.archiveSize` (`asset-utils.ts:54-57`). A recipient with no user session omits it. The server then uses 4 GiB (`reference/immich/server/src/services/download.service.ts:39`). A body with no `assetIds`, `albumId`, or `userId` is 400 `assetIds, albumId, or userId is required` (`download.service.ts:35-37`).
- Response fields the page requires: `archives`. A missing `archives` throws on `.length` after the 201 is accepted, and the click handlers do not catch that throw (`asset-utils.ts:68`). An empty array finishes with no toast and no file. Each item's `assetIds` is posted to the archive route. A missing `assetIds` throws inside the loop, the page toasts `errors.unable_to_download_files`, and it stops (`asset-utils.ts:95-98`). `size` is the toast text and, when there is more than one archive, the panel label (`asset-utils.ts:87-91`, `reference/immich/web/src/routes/DownloadPanel.svelte:36-38`). `totalSize` is not read. A thrown info call toasts the same error and returns (`asset-utils.ts:58-61`).
- Source: `asset-utils.ts:54-100`, `download.dto.ts:4-25`, `download.controller.ts:16-26`, `download.service.ts:20-87`.

### `POST /api/download/archive?key=` or `?slug=`

- When: after a successful info call. One archive submits immediately. More than one archive waits for a click on the download panel (`asset-utils.ts:82-85`, `DownloadPanel.svelte:11-14`).
- Method and path: `POST /api/download/archive` as a browser form, not the SDK (`utils.ts:296-316`). The controller answers 200 (`download.controller.ts:28-39`). The route allows a share key.
- Request: form fields `assetIds` as one comma-separated string, `archiveName`, and `edited` set to the string `true` (`utils.ts:310-312`). The page always sends `edited`. The query is the share `key` or `slug`.
- Response fields the page requires: none. The browser saves the body. The page does not read JSON.
- Source: `utils.ts:296-316`, `download.controller.ts:28-39`, `download.dto.ts:27-50`.

### `GET /api/assets/{id}?key=` or `?slug=`

- When: the page load of `/photos/{id}`, the album viewer's next and previous Assets, a one-Asset selection download, and a slideshow start that calls `setAssetId`. The client accepts only 200 (`fetch-client.ts:4859-4872`).
- Request: no body. `id` is the Asset id. The share query is `key` or `slug`.
- Response fields the page requires: the opened viewer reads the same fields as the one-Asset case in [Shared link open](shared-link-open.md): `type`, `id`, `thumbhash`, `width`, `height`, `livePhotoVideoId`, `exifInfo.projectionType`, `originalPath`, and `hasMetadata`. A single-file download also reads `originalFileName`, `id`, `thumbhash`, and `livePhotoVideoId`. When `livePhotoVideoId` is set it fetches that Asset and reads `originalPath` and `originalFileName` (`asset.service.ts:393-426`). `originalPath` missing throws on `.includes` before any file is saved. A missing `id` on the opened Asset builds media URLs with `undefined`.
- The page load passes `updateCache` false, so that response is not stored (`reference/immich/web/src/lib/utils/navigation.ts:23-25`, `reference/immich/web/src/lib/managers/AssetCacheManager.svelte.ts:12-25`, `54-55`). The neighbor fetches use the default and are stored.
- Source: `shared-links.ts:40`, `+layout.svelte:14-18`, `TimelineAssetViewer.svelte:42-78`, `asset-viewer-manager.svelte.ts:253-256`, `asset.service.ts:393-426`, `fetch-client.ts:4859-4872`.

### `GET /api/assets/{id}/original?edited=true&key=` or `&slug=`

- When: a one-Asset download, including the motion part of a live photo (`asset.service.ts:436`). Also the opened viewer when it asks for the Original: a still whose preview errors or whose zoom passes 1, a video whose local preference `always-load-original-video` is true (the stored default is false, `preferences.store.ts:136`), and a panorama's original slot.
- Method and path: `GET /api/assets/{id}/original` (`utils.ts:234-238`). `size` is omitted on this path. `edited` defaults to true in `getAssetMediaUrl`. `c` is the thumbhash when it is set. The share query is included.
- A shared-link session forces `edited` true on this route even when the query says otherwise (`reference/immich/server/src/services/asset-media.service.ts:229-234`). The viewer's single-file download already sends true.
- Response fields the page requires: the body is the file. A download gives it `originalFileName` as the saved name (`asset.service.ts:436`). The viewer does not read JSON.
- A still whose link has `allowDownload` false or `showMetadata` false never uses this URL for the photo viewer. Both the preview slot and the original slot are `size=preview` (`utils.ts:206-210`, `213-218`). A video does not read those two flags. Its player uses playback, HLS, or this Original URL (`reference/immich/web/src/lib/components/asset-viewer/VideoNativeViewer.svelte:80-89`).
- Source: `asset.service.ts:393-440`, `utils.ts:192-238`, `asset-media.service.ts:229-247`.

### `GET /api/assets/{id}/thumbnail?size=&edited=true&key=` or `&slug=`

- When: an opened still, and the preload of the next and previous stills (`reference/immich/web/src/lib/components/asset-viewer/PhotoViewer.svelte:228-230`, `reference/immich/web/src/lib/components/AdaptiveImage.svelte:105-120`, `reference/immich/web/src/lib/components/asset-viewer/PreloadManager.svelte.ts:16-41`). The grid thumbnail already loaded on the open.
- Request: `size=thumbnail`, then `size=preview`, with `edited=true` and the share query. `c` is the thumbhash when it is set. Preload uses the same pair, then the Original URL when the preview errors.
- When `allowDownload` is false or `showMetadata` is false, the preview and original slots are both `size=preview` (`utils.ts:206-210`). The stored default of `always-load-original-file` is false (`preferences.store.ts:128`). A still with `type` image and a `duration` asks for the Original or fullsize unless those two link flags force preview (`utils.ts:221-231`).
- An equirectangular still, or an `originalPath` that ends in `.insp`, uses `viewAsset` instead of the image element: `GET` this path with `size=preview` and no `edited` (`reference/immich/web/src/lib/components/asset-viewer/ImagePanoramaViewer.svelte:17-19`, `fetch-client.ts:5021-5038`). The client accepts only 200. The server default for a missing `edited` is false (`reference/immich/server/src/dtos/asset-media.dto.ts:28`). Its original slot is `getAssetUrl` with `forceOriginal` and no shared-link argument, so the allow-download check does not apply (`ImagePanoramaViewer.svelte:27`, `utils.ts:206-210`).
- Response fields the page requires: the body is the image. A failed image is the broken placeholder. `id` missing builds a URL with `undefined`.
- Source: `AdaptiveImage.svelte:105-124`, `utils.ts:192-238`, `PreloadManager.svelte.ts:16-41`, `ImagePanoramaViewer.svelte:17-27`.

### `GET /api/assets/{id}/video/playback?key=` or `&slug=`

- When: hover, as above. Also an opened video while `realtimeTranscoding` is false and the Original preference is off (`VideoNativeViewer.svelte:80-89`). An equirectangular video uses this URL with no `c` (`reference/immich/web/src/lib/components/asset-viewer/VideoPanoramaViewer.svelte:27`).
- This is the playback route already named. The body is the Original. Status is 200, or 206 when the player sends `Range`. Hover and this viewer do not request `GET /api/assets/{id}/video/stream/main.m3u8` while `realtimeTranscoding` is false. Hover does not read the flag at all (`utils.ts:241-243`).
- A live photo played from the viewer uses the same playback URL with `livePhotoVideoId` (`AssetViewer.svelte:570-574`).
- Response fields the page requires: the body is the video. A hover error shows the alert icon when `src` is non-empty (`VideoThumbnail.svelte:78-84`). `duration` missing on the timeline Asset uses 0 (`Thumbnail.svelte:262`).
- Source: `Thumbnail.svelte:255-277`, `VideoThumbnail.svelte:37-84`, `VideoNativeViewer.svelte:80-89`, `utils.ts:241-247`.

### `GET /api/timeline/bucket?timeBucket=&albumId=&order=&key=` or `&slug=`

- When: a shuffle slideshow picks a month that is not loaded (`reference/immich/web/src/lib/managers/timeline-manager/timeline-manager.svelte.ts:468-485`). The ordered album start uses the first loaded Asset and does not fetch a month (`AlbumViewer.svelte:49-58`). If that month has no loaded Asset, the click does nothing. Opening an Asset whose month is not loaded also fetches that month (`Timeline.svelte:510-516`).
- The path, query, and required bucket arrays are the ones in [Shared link open](shared-link-open.md). This action does not add a field.
- Source: `AlbumViewer.svelte:49-58`, `timeline-manager.svelte.ts:468-485`, `Timeline.svelte:510-516`.

### `POST /api/assets?key=` or `?slug=`

- When: the add button or a drop, after the extension check. The page does not call `POST /api/assets/bulk-upload-check` on a shared link (`reference/immich/web/src/lib/utils/file-uploader.ts:178-197`). It does not call add-to-album (`file-uploader.ts:222-225`).
- Method and path: `POST /api/assets` with the share query (`file-uploader.ts:199-207`). The route allows a share key (`reference/immich/server/src/controllers/asset-media.controller.ts:51-53`). The page accepts 200 and 201. The server uses 201 for a new Asset and 200 for a duplicate (`asset-media.controller.ts:61-87`).
- Request: multipart fields `fileCreatedAt`, `fileModifiedAt`, `isFavorite` `false`, and `assetData` (`file-uploader.ts:164-171`). Both dates are `lastModified` as ISO. The page sends no checksum, no `albumId`, no `duration`, and no `filename`. `deviceAssetId` stays in the browser queue (`file-uploader.ts:112-114`).
- The extension list is `image` and `video` from `GET /api/server/media-types`, which the root layout fires at app init, before this click (`reference/immich/web/src/lib/managers/upload-manager.svelte.ts:21-30`, `reference/immich/web/src/routes/+layout.svelte:178`, `fetch-client.ts:6826-6832`). That route is public (`reference/immich/server/src/controllers/server.controller.ts:140-148`). The body is `{ image, video, sidecar }` (`reference/immich/server/src/services/server.service.ts:168-173`). A failed load leaves the arrays empty, and every file toasts `unsupported_file_type` with no POST (`file-uploader.ts:101-105`). `sidecar` is not used for this check.
- Response fields the page requires: `status` and `id`. `status` `duplicate` marks the queue row duplicated and leaves it up. Any other accepted status marks it done and removes it after one second (`file-uploader.ts:216-237`). `isTrashed` is copied when present. A missing `id` is stored as `undefined`. Any other status toasts `errors.unable_to_upload_file` (`file-uploader.ts:209-210`, `247-250`). The individual page then toasts an empty success, or `errors.unable_to_add_assets_to_shared_link` (`IndividualSharedViewer.svelte:50-59`). The album page does not add that toast.
- The page does not refetch the link. The server attaches the new id, or the duplicate id, to the individual link or to the link's album (`asset-media.service.ts:189-191`, `211-212`, `348-359`). A link with `allowUpload` false still gets the POST from a drop. `requireUploadAccess` then throws 401 (`reference/immich/server/src/utils/access.ts:38-41`, `asset-media.service.ts:105`). The page toasts the upload error.
- Source: `file-uploader.ts:150-253`, `asset-media.controller.ts:51-87`, `asset-media.service.ts:189-216`, `348-359`, `upload-manager.svelte.ts:21-30`.

### Map style URL

- When: the modal has loaded `Map` (`reference/immich/web/src/lib/modals/MapModal.svelte:20-28`). This is not an `/api` route.
- The style is `mapDarkStyleUrl` or `mapLightStyleUrl` from the config already loaded on the open (`reference/immich/web/src/lib/components/shared-components/map/Map.svelte:113-116`). Those two fields are `config.map.darkStyle` and `config.map.lightStyle` (`reference/immich/server/src/services/server.service.ts:128-129`). The defaults are `https://tiles.immich.cloud/v1/style/dark.json` and `https://tiles.immich.cloud/v1/style/light.json` (`reference/immich/server/src/dtos/config.dto.ts:650-653`). The modal does not call `GET /api/server/config` again.
- Marker fields the page reads: `id`, `lat`, and `lon` (`Map.svelte:189-199`). `city`, `state`, and `country` are copied onto the feature. A missing one is `undefined` on that feature. A click passes `id` to navigation (`AlbumMap.svelte:53-56`, `Map.svelte:131-135`).
- Source: `AlbumMap.svelte:53-61`, `MapModal.svelte:16-29`, `Map.svelte:113-116`, `189-199`.

## Not one of these actions

Cast is the toolbar button on an album link. It opens a Google Cast dialog when a Cast destination is already available (`reference/immich/web/src/lib/services/app.service.ts:8-16`, `AlbumViewer.svelte:116`). It makes no Immich HTTP call.

Remove from the link is the individual multi-select bar, and only when the signed-in User owns the link (`IndividualSharedViewer.svelte:102-104`). Confirm calls `DELETE /api/shared-links/{id}/assets` with `{ assetIds }` (`reference/immich/web/src/lib/services/shared-link.service.ts:125-140`, `fetch-client.ts:7085-7096`). The client accepts only 200. The page reads each result's `success` and `assetId`, and drops those Assets from the in-memory list. Cancel sends nothing.
