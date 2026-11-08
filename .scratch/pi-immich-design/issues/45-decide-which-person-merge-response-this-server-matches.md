# Decide which person merge response this server matches

Type: grilling
Status: resolved

## Question

The web people page merges people with `POST /api/people/merge`. The mobile app does not send that call. The call shape is in [Remaining to close](../../../docs/research/remaining-to-close.md). Where the web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Does that merge match v3.3.6?

## Comments

Recommendation accepted: `POST /api/people/merge` matches v3.3.6. The phone does not send it.

## Answer

`POST /api/people/merge` matches v3.3.6. The body is `{ ids }`, with the person who remains first and each person merged into them after that. The response is 200 and one `{ id, success, error? }` per id, in that order. `error` is `no_permission`, `not_found`, or `unknown`. The page counts `success` and then reloads the remaining person with `GET /api/people/{id}`. Fewer than two ids is 400 `At least two people are required for merging`. A repeated id is 400 `Cannot merge a person into themselves`.

The first id that the caller can merge is the person who remains. Each later id that the caller can merge moves its faces onto that person, copies a name or birthday only when the remaining person does not already have one, and deletes the merged person. The deleted person's thumbnail file is removed. Faces sync on `AssetFaceV3` under the remaining person. The deleted person syncs as a person delete. An id the caller cannot merge is `{ success: false, error: "no_permission" }` and is left in place. The remaining person's own entry is `{ success: true }` when that person exists for the caller.

The same merge runs for another User who already has one of these people, and it skips that User when both copies already have a name or both already have a birthday. This server still creates no cluster, so a person belongs to the User who owns it and that second pass finds nobody else. The phone does not send this call. It stores the sync that follows.
