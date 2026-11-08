# Decide which person hide and favorite responses this server matches

Type: grilling
Status: resolved

## Question

The web people page writes `PUT /api/people/{id}` with `isHidden` or `isFavorite`. The mobile people screen stores those flags from sync and does not send the write. The call shapes are in [Remaining to close](../../../docs/research/remaining-to-close.md). Where the web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Does that PUT match v3.3.6?

## Comments

Recommendation accepted: `PUT /api/people/{id}` matches v3.3.6 for hide and favorite. The phone still only stores the flags from sync.

## Answer

`PUT /api/people/{id}` matches v3.3.6. Hide sends `{ isHidden: true }` or `{ isHidden: false }`. Favorite sends `{ isFavorite: true }` or `{ isFavorite: false }`. The response is 200 and the person document: `id` (the person group id), `name`, `birthDate`, `thumbnailPath`, `isHidden`, `isFavorite`, `updatedAt`, `color`, `otherPeople`, `sharedBy`, and `sharedWith`. The page emits `PersonUpdate` with that document and reads `isHidden` and `isFavorite`. Name and birthday on this same route stay as [Inventory how the app reads ML results](06-inventory-how-the-app-reads-ml-results.md) already decided.

The flags are stored on that caller's person row. The next `PeopleV1` sync includes `isHidden` and `isFavorite`, and the phone stores both. The phone does not send this write.

A missing person is 400 `Not found or no person.update access`. Sending `isHidden` or `isFavorite` for another User's copy of the person is 400 `Only name and birthDate can be updated for other users`. A session that is not signed in is 401. A shared-link session is 403.
