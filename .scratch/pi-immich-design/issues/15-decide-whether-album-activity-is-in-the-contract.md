# Decide whether album activity is in the Contract

Type: grilling
Status: resolved

## Question

The album options screen sends `isActivityEnabled` on `PATCH /api/albums/{id}`. The comments screen uses the activities client: list, create a like or comment, and delete (`reference/immich/mobile/lib/data/server/activity.dart`). [Inventory albums, trash, and shared links](05-inventory-albums-trash-and-shared-links.md) records the switch and leaves those calls uninventoried. The success path names albums, not comments or likes. Is album activity part of the Contract?

## Comments

Recommendation accepted: album activity is in the Contract. That is the comments-and-likes switch, and likes and comments on a shared album and on an Asset in that album.

## Answer

Album activity is in the Contract. The owner sends `isActivityEnabled` on `PATCH /api/albums/{id}`. On a shared album with that switch on, the comments screen lists, creates, and deletes a like or a comment. A like or a comment may name one Asset in the album.

An Activity is a like or a comment a User leaves on a shared album, or on an Asset in that album. The Pi can store those rows. None of the eight constraints in [Decide which Pi constraints the app can see](08-decide-which-pi-constraints-the-app-can-see.md) remove these calls.

The method, path, and fields are [Inventory album activity](25-inventory-album-activity.md). [Inventory albums, trash, and shared links](05-inventory-albums-trash-and-shared-links.md) recorded the switch and left the activities client out.
