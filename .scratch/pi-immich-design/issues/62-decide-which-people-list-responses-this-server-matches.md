# Decide which people list responses this server matches

Type: grilling
Status: resolved

## Question

The phone reads people from sync. The signed-in web calls `GET /api/people`, `GET /api/people/{id}`, `GET /api/people/{id}/statistics`, `GET /api/search/person`, and `GET`, `PUT`, and `DELETE /api/people/users`. Name, birthday, hide, favorite, merge, and face reassignment are already decided. Where the official web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do these people reads and the people-user writes match v3.3.6?

## Comments

Recommendation accepted: the people reads and the people-user writes match v3.3.6. Name search cannot use the Postgres trigram operator, so which names match can differ. Sharing a person requires a cluster group, which creating a User still does not add.

## Answer

The people reads and the people-user writes match v3.3.6. The one Pi difference is name-search matching. An unsigned call is 401. A shared-link session is 403 `Forbidden`.

A person document is `id` (the `personGroupId`), `name`, `birthDate`, `thumbnailPath`, `isHidden`, `isFavorite`, `color`, `updatedAt`, `otherPeople`, `sharedBy`, and `sharedWith`. The people page reads `id`, `name`, `isHidden`, `isFavorite`, `birthDate`, and `updatedAt`.

`GET /api/people` returns 200 and `{ people, total, hidden, hasNextPage }`. The people page sends `withHidden=true`. Explore sends `withHidden=false`. The default page is 1 and the default size is 500. `name`, `isFavorite`, `isHidden`, `sharedById`, and `sharedWithId` filter the list. No people is `people: []` with `total` 0. The list includes people from partners who shared with the caller, whether or not `inTimeline` is true.

`GET /api/people/{id}` returns 200 and one person document. A missing person is 400 `Not found or no person.read access`.

`GET /api/people/{id}/statistics` returns 200 and `{ assets }`. The person page reads `assets`. The count includes the same partners. A missing person is the same 400.

`GET /api/search/person` returns 200 and up to 100 person documents for the caller. Hidden people are omitted unless `withHidden` is true. This server does not use the Postgres trigram operator, so which names match, and their order, can differ from v3.3.6.

`person_user` is `(personGroupId, sharedById, sharedWithId)` with `role`, `createdAt`, `updatedAt`, and `updateId`. `sharedById` and `sharedWithId` cannot be the same user.

`GET /api/people/users` returns 200 and `{ personId, sharedById, sharedWithId, sharedBy, sharedWith, role }`. No shares is `[]`.

`PUT /api/people/users` returns 200 and an empty body. The body needs `sharedWithIds`, `role`, and either `personIds` or `type`. Sharing with yourself is 400 `Cannot share a person with yourself`. A user outside the caller's cluster group is 400 `All users must be in the same cluster group`. Creating a User still does not add a cluster, so this call fails that way until a group exists. `DELETE /api/people/users` returns 200 and an empty body and drops those shares.

Name, birthday, hide, favorite, merge, and face reassignment stay on their tickets. The thumbnail stays on [Inventory where the person thumbnail lives](18-inventory-where-the-person-thumbnail-lives.md).
