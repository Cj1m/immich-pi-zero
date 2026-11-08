# Decide which favorite responses this server matches

Type: grilling
Status: resolved

## Question

The phone's favorite action writes `PUT /api/assets` with `isFavorite` and accepts 204. The web heart writes `PUT /api/assets/{id}` with `{ isFavorite }` and accepts 200. Both write `asset.isFavorite`. The call shapes are in [Remaining to close](../../../docs/research/remaining-to-close.md). Where the official mobile app or the official web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do both of those writes match v3.3.6?

## Comments

Recommendation accepted: both favorite writes match v3.3.6. Each one sets `asset.isFavorite`.

## Answer

Both writes match v3.3.6. Each one sets `asset.isFavorite`. The timeline tile already reads that flag from sync. A change bumps the asset's sync cursor, so the next sync sends the new value. The phone also writes its local row when its own call returns.

`PUT /api/assets` is the phone. The favorite action sends `{ ids, isFavorite }`. `isFavorite` is boolean. `ids` is the selected asset ids. The response is 204 with an empty body. The phone reads no field. An id the caller cannot update is 400 `Not found or no asset.update access`, and nothing is written. A shared-link session is 403. After a favorite-only change the server sends `on_asset_update` to that User for each asset. It does not queue a sidecar write.

`PUT /api/assets/{id}` is the web heart. The body is `{ isFavorite: true }` or `{ isFavorite: false }`. The response is 200 and the asset document, with `isFavorite` set to that value. The page emits `AssetUpdate` with that document and shows the toast. It reads `isFavorite` from the document. The same access rules apply. A missing asset is 400 `Asset not found`. This call also sends `on_asset_update` to the caller.

The bulk route is the one the phone already uses for a favorite. The single-asset route is the web heart. Other fields those DTOs allow stay on the screens that send them.
