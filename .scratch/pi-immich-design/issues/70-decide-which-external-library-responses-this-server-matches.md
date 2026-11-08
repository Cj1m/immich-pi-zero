# Decide which external-library responses this server matches

Type: grilling
Status: resolved

## Question

The admin library screen calls `GET` and `POST /api/libraries`, `GET`, `PUT`, and `DELETE /api/libraries/{id}`, `POST /api/libraries/{id}/validate`, `GET /api/libraries/{id}/statistics`, and `POST /api/libraries/{id}/scan`. [Decide which asset and exif columns this server stores](issues/52-decide-which-asset-and-exif-columns-this-server-stores.md) says `libraryId` stays null. Where the official web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do these library calls match v3.3.6?

## Comments

Recommendation accepted: the library document, the path check, and the statistics match v3.3.6. A scan does not import a folder, and an upload still leaves `libraryId` null. This server does not watch a folder.

## Answer

`library` keeps `id`, `name`, `ownerId`, `importPaths` (JSON text), `exclusionPatterns` (JSON text), `createdAt`, `updatedAt`, `deletedAt`, `refreshedAt`, and `updateId`. The document is `id`, `ownerId`, `name`, `assetCount`, `importPaths`, `exclusionPatterns`, `createdAt`, `updatedAt`, and `refreshedAt`. `assetCount` is the number of assets with that `libraryId`. On this server that count stays 0.

`GET /api/libraries` returns 200. A soft-deleted library is left out. None is `[]`. `POST /api/libraries` returns 201. An omitted name is `New External Library`. Omitted import paths are `[]`. Omitted exclusion patterns are `**/@eaDir/**`, `**/._*`, `**/#recycle/**`, `**/#snapshot/**`, `**/.stversions/**`, and `**/.stfolder/**`. `GET /api/libraries/{id}` returns 200. A missing library is 400 `Library not found`. `PUT /api/libraries/{id}` returns 200. An import path that fails the check below is 400 `Invalid import path: {message}`. `DELETE /api/libraries/{id}` returns 204, sets `deletedAt`, and then removes the row. Assets stay, because none carry that `libraryId`.

`POST /api/libraries/{id}/validate` returns 200 `{importPaths}`. Each item is `{importPath, isValid, message}`. A path inside the media upload folder is `Cannot use media upload folder for external libraries`. A relative path is `Import path must be absolute, try {absolute}`. A missing path is `Path does not exist (ENOENT)`. A file is `Not a directory`. A directory the process cannot read is `Lacking read permission for folder`. A path that passes is `isValid` true.

`GET /api/libraries/{id}/statistics` returns 200 `{photos, videos, total, usage}`. A missing library is 400 `Library {id} not found`. With no library assets the four numbers are 0.

`POST /api/libraries/{id}/scan` returns 204. It does not read a folder and does not create assets. `libraryId` on an upload stays null. The library watch switch does not start a watcher. A missing library is 400 `Library not found`.

A non-admin is 403. Unsigned is 401. A shared link is 403 `Forbidden`.
