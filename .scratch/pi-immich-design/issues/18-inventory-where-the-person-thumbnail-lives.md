# Inventory where the person thumbnail lives

Type: research
Status: resolved

## Question

Where does the v3.3.6 server in `reference/immich` store the file that `GET /api/people/{id}/thumbnail` reads? Name the column or path, and the job that writes it. [Inventory how the app reads ML results](06-inventory-how-the-app-reads-ml-results.md) records the request. This ticket is where those bytes live. Write the note to `docs/research/person-thumbnail.md` and link it from this ticket when resolving.

## Answer

The GET opens `person.thumbnailPath`. The column is a string on `person`, default `''`. The writer stores the absolute path `{mediaLocation}/thumbs/{ownerId}/{personGroupId[0:2]}/{personGroupId[2:4]}/{personGroupId}.jpeg`. `sendFile` uses that stored string. It does not recompute the formula.

The media location is `IMMICH_MEDIA_LOCATION` when that absolute path is set. Otherwise the server uses `/data` or `/usr/src/app/upload` when exactly one of those directories exists, and `/usr/src/app/upload` otherwise.

`PersonGenerateThumbnail` writes the file. It crops the featured face from the Original, or from the preview when the asset is a video, to a 250px JPEG, and sets the column to that path. A second run overwrites the same file. A person with no usable face is not given one.

A missing person, an empty path, and a file that is not readable are 404. A shared-link session is 403. An empty path on the viewer's own row falls through to one other owner's non-empty `thumbnailPath` in the same group.

Directories, the column, and the job are in [Person thumbnail](../../../docs/research/person-thumbnail.md).
