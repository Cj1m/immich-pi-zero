# Decide which partner responses this server matches

Type: grilling
Status: resolved

## Question

The web sharing screen uses `GET /api/partners`, `POST /api/partners`, `PUT /api/partners/{id}`, and `DELETE /api/partners/{id}`. The mobile timeline already shows partner assets that arrived on the sync stream. The call shapes are in [Remaining to close](../../../docs/research/remaining-to-close.md). Where the official mobile app or the official web UI can see a behavior, match v3.3.6. A difference is allowed when the Pi cannot do it. Do those partner calls match v3.3.6?

## Comments

Recommendation accepted: all four partner calls match v3.3.6. The phone sends the create, the timeline toggle, and the delete. The web sends all four. The Pi can store the row.

## Answer

All four calls match v3.3.6. There is no Pi difference.

`GET /api/partners?direction=` is required. `shared-by` lists rows where the caller is `sharedById` and returns the other User. `shared-with` lists rows where the caller is `sharedWithId` and returns the other User. The response is 200 and one partner document per row. A partner document is the user document (`id`, `name`, `email`, `profileImagePath`, `avatarColor`, `profileChangedAt`) plus `inTimeline`. Sharing settings reads `id`, `name`, `email`, `inTimeline`, and the avatar fields. The sharing page reads `id`, `name`, `email`, and the avatar fields. The partner picker reads `id`. A soft-deleted User is left off the list. A missing or unknown `direction` is 400.

`POST /api/partners` with `{ sharedWithId }` creates `{ sharedById: caller, sharedWithId }` with `inTimeline` false. The response is 201 and the other User plus `inTimeline`. The web reloads the list and does not read the body. The phone requires a partner document and then stores the row locally with `inTimeline` false. A duplicate is 400 `Partner already exists`. A missing or soft-deleted User is 400 `Invalid user`. The deprecated `POST /api/partners/{id}` is that same create, with the path id as `sharedWithId`. The web and the phone send the body form.

`PUT /api/partners/{id}` with `{ inTimeline }` updates the row where `sharedById` is the path id and `sharedWithId` is the caller. That is the recipient choosing whether the sharer's assets show on their timeline. The response is 200 and the sharer plus `inTimeline`. The web and the phone set the switch from the request. Neither reads a response field. A missing row is 400 `Not found or no partner.update access`.

`DELETE /api/partners/{id}` removes the row where `sharedById` is the caller and `sharedWithId` is the path id. The response is 204 and empty. A missing row is 400 `Partner not found`. The web reloads the list. The phone drops its local row.

An unsigned call is 401. A shared-link session is 403 `Forbidden`.

The next sync sends `PartnerV1` with `sharedById`, `sharedWithId`, and `inTimeline` to both Users. A delete sends `PartnerDeleteV1` with those two ids. Once the row exists, the recipient's sync includes the sharer's assets. The timeline still shows those assets only when `inTimeline` is true, as [Inventory the timeline](03-inventory-the-timeline.md) already decided.
