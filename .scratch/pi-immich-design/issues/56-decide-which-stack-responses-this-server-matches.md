# Decide which stack responses this server matches

Type: grilling
Status: resolved

## Question

[Decide which duplicate responses this server matches](48-decide-which-duplicate-responses-this-server-matches.md) creates a stack with `POST /api/stacks` and then clears `duplicateId`. [Inventory the timeline](03-inventory-the-timeline.md) already reads the stack primary. v3.3.6 also has `GET`, `PUT`, and `DELETE` on `/api/stacks`, and removing one Asset from a stack. Where the official mobile app or the official web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do those stack calls match v3.3.6?

## Comments

Recommendation accepted: the stack calls match v3.3.6. The phone creates and deletes a stack. The web also changes the primary and removes one Asset.

## Answer

The stack calls match v3.3.6. There is no Pi difference.

`stack` keeps `id`, `createdAt`, `updatedAt`, `updateId`, `primaryAssetId`, and `ownerId`. `primaryAssetId` is unique. Each Asset on the stack stores that id in `stackId`. A deleted stack writes `stack_audit` with `id`, `stackId`, `userId`, and `deletedAt`. Deleting the stack clears `stackId` on its Assets.

A stack document is `id`, `primaryAssetId`, and `assets`. The primary Asset is first. Each asset is the asset document. The phone reads `id`, `primaryAssetId`, and the asset ids. The web reads `primaryAssetId` and `assets`.

`POST /api/stacks` with `{ assetIds }` returns 201 and that document. The first id is the primary. Fewer than two ids is 400. An Asset the caller cannot update is 400 `Not found or no asset.update access`. When one of those ids is already a stack primary, that stack's Assets join this stack and the old stack is deleted. The next sync sends `StackV1` with `id`, `createdAt`, `updatedAt`, `primaryAssetId`, and `ownerId`.

`PUT /api/stacks/{id}` with `{ primaryAssetId }` returns 200 and the stack document. The web reads it. The primary must be an Asset already on the stack, or the response is 400 `Primary asset must be in the stack`. A missing stack is 400 `Not found or no stack.update access`. `PATCH /api/stacks/{id}` is the same update. The web sends the PUT.

`DELETE /api/stacks` with `{ ids }` and `DELETE /api/stacks/{id}` return 204 and an empty body. The phone and the web send the bulk form. A missing stack is 400 `Not found or no stack.delete access`. The next sync sends `StackDeleteV1` as `{ stackId }`.

`DELETE /api/stacks/{id}/assets/{assetId}` returns 204 and an empty body. The Asset leaves the stack. The primary cannot be removed: 400 `Cannot remove stack's primary asset`. An Asset that is not on that stack is 400 `Asset not in stack`.

`GET /api/stacks` returns 200 and the caller's stacks. An optional `primaryAssetId` limits the list to that primary. `GET /api/stacks/{id}` returns 200 and one stack document. A missing stack is 400 `Not found or no stack.read access`. The phone and the stack actions use the create response and the sync, not these two reads.

An unsigned call is 401. A shared-link session is 403 `Forbidden`. The timeline still shows the stack primary, as [Inventory the timeline](03-inventory-the-timeline.md) already decided.
