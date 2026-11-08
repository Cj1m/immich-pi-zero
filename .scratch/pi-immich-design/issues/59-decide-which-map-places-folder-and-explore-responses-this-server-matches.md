# Decide which map, places, folder, and explore responses this server matches

Type: grilling
Status: resolved

## Question

The signed-in map, places, folders, explore, and large-files screens call `GET /api/map/markers`, `GET /api/search/cities`, `GET /api/search/explore`, `GET /api/search/places`, `POST /api/search/large-assets`, `GET /api/view/folder`, and `GET /api/view/folder/unique-paths`. The search bar also calls `GET /api/search/suggestions`. Where the official web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do those calls match v3.3.6?

## Comments

Recommendation accepted: these reads match v3.3.6. Place search cannot use the Postgres trigram operator, so its order can differ. The folder tree is the stored `originalPath`.

## Answer

These reads match v3.3.6. The one Pi difference is place-search order. An unsigned call is 401. A shared-link session is 403 `Forbidden`.

`GET /api/map/markers` returns 200 and `{ id, lat, lon, city, state, country }` for assets that have coordinates and are not deleted, newest `fileCreatedAt` first. The map reads those fields. With no archive flag, only timeline visibility is included. `isArchived=true` also includes the caller's own archived assets. `isFavorite` keeps that flag. `fileCreatedAfter` and `fileCreatedBefore` bound `fileCreatedAt`. `withPartners` true adds every partner who shared with the caller, including a partner whose `inTimeline` is false. `withSharedAlbums` true also includes assets in albums the caller can see. No located assets is `[]`.

`GET /api/search/cities` returns 200 and one asset document per city. The asset is a timeline image, not deleted. The places page reads the city and the asset id. Partners with `inTimeline` true are included. No city is `[]`.

`GET /api/search/explore` returns 200 and two groups. The page reads `fieldName` and `items`. The first `fieldName` is `exifInfo.city`. Each item is `{ value: city, data: asset }` for a timeline image in that city. A city needs at least five of the caller's images, and at most twelve cities are returned. The second `fieldName` is `createdAt`. Each item is one of the twelve newest timeline images, and `value` is that asset's `createdAt`. Partners are not included. A caller with nothing matching gets empty `items`.

`GET /api/search/places?name=` returns 200 and up to twenty `{ name, latitude, longitude, admin1name, admin2name }`. The geolocation picker reads `name`, `latitude`, and `longitude`. A place matches when the query hits its name, admin1 name, admin2 name, or alternate names. No hit is `[]`. The gazetteer is the v3.3.6 places table, stored in SQLite. This server does not use the Postgres trigram operator, so the order can differ from v3.3.6. A missing `name` is 400.

`POST /api/search/large-assets` returns 200 and asset documents, largest `fileSizeInByte` first. The page sends `minFileSize: 0` and reads the asset document. The default `size` is 250. The file must be larger than `minFileSize`. Hidden assets are left out unless visibility is hidden. Partners with `inTimeline` true are included. Locked visibility without an unlocked session is 401 `Elevated permission is required`.

`GET /api/view/folder/unique-paths` returns 200 and the directory of each `originalPath`, trailing slash removed, ascending. `GET /api/view/folder?path=` returns 200 and the asset documents that sit directly in that directory, filename ascending. Both keep the caller's timeline assets that are not deleted and have `fileCreatedAt`, `fileModifiedAt`, and `localDateTime`. The tree is that stored path. It is not a second folder index.

`GET /api/search/suggestions` returns 200 and the distinct strings for `type` `country`, `state`, `city`, `camera-make`, `camera-model`, or `camera-lens-model`. The search bar reads the strings. `includeNull=true` adds a null entry. Partners with `inTimeline` true are included. No values is `[]`.
