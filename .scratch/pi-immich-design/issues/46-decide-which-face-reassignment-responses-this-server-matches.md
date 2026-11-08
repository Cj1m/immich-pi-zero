# Decide which face reassignment responses this server matches

Type: grilling
Status: resolved

## Question

The web people page moves faces with `PUT /api/people/{id}/reassign` and `PUT /api/faces/{id}`, and removes one face with `DELETE /api/faces/{id}`. The mobile app does not send those calls. The call shapes are in [Remaining to close](../../../docs/research/remaining-to-close.md). Where the web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do those three calls match v3.3.6?

## Comments

Recommendation accepted: the bulk reassign, the single-face reassign, and the face delete match v3.3.6. The same screens create an empty person first when the face is moving to someone new.

## Answer

All three calls match v3.3.6. The phone does not send them. It stores the face and person sync that follows.

`PUT /api/people/{id}/reassign` moves every face of `personId` on `assetId` onto the person in the path. The body is `{ data: [{ assetId, personId }] }`. The response is 200 and one person document per item in `data`, each the destination person from [Decide which person hide and favorite responses this server matches](44-decide-which-person-hide-and-favorite-responses-this-server-matches.md). The page reads no field. A missing destination person is 400 `Person not found`.

`PUT /api/faces/{id}` moves that one face onto the person in the path. The body is `{ id }` of the face, which is the same id as the path. The response is 200 and the destination person document. The page reads no field. A missing face is 400 `Not found or no person.create access`.

`DELETE /api/faces/{id}` with `{ force: false }` sets that face's `deletedAt` and returns 204 with an empty body. The page drops the face locally. `{ force: true }` removes the row. The web sends `force: false`. A soft-deleted face syncs as a face delete. The box stays out of the phone's drawing, as [Decide whether face boxes are in the Contract](16-decide-whether-face-boxes-are-in-the-contract.md) already decided.

Both reassigns set the face's person to the destination. `AssetFaceV3` then carries that person. When the destination person has no thumbnail face, or the moved face was the old person's thumbnail face, the server queues `PersonGenerateThumbnail` for that person.

The reassign screens create the new person with `POST /api/people` and `{}` before the move. That returns 201 and the person document. The page reads `id`. The new person has an empty name, `isHidden` false, `isFavorite` false, and an empty thumbnail path.
