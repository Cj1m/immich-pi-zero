# Decide which shared-link columns this server stores

Type: grilling
Status: resolved

## Question

[Decide which shared-link open responses this server matches](33-decide-which-shared-link-open-responses-this-server-matches.md) and [Decide which shared-link actions after open this server matches](39-decide-which-shared-link-actions-after-open-this-server-matches.md) match the responses. The columns those bodies read are listed in [Remaining to close](../../../docs/research/remaining-to-close.md). v3.3.6 stores them in Postgres. This server stores them in SQLite. Which columns does this server keep for a shared link and for the assets on that link?

## Comments

Recommendation accepted: keep the v3.3.6 shared-link columns. Postgres `bytea`, `uuid`, and `timestamptz` become SQLite blob, text, and text.

## Answer

`shared_link` keeps `id`, `description`, `userId`, `key`, `type`, `createdAt`, `expiresAt`, `allowUpload`, `albumId`, `allowDownload`, `showExif`, `password`, and `slug`.

`id`, `userId`, and `albumId` are text. `key` is a blob, unique. `slug` is text, unique, and nullable. `createdAt` and `expiresAt` are text. `description` and `password` are text and nullable. `allowUpload` defaults to false. `allowDownload` and `showExif` default to true. The API name for `showExif` is `showMetadata`, as those responses already use.

`shared_link_asset` keeps `assetId` and `sharedLinkId`. Both are text. The pair is the primary key. Deleting the link or the Asset removes the pair.

No other shared-link column is stored.
